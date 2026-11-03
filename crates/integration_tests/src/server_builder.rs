//! `GrpcServer::builder` tests against a live in-process server.
//!
//! Every server listens on an OS-assigned port. Tests that assert on spans
//! run on the default current-thread runtime so that the thread-local
//! subscriber from [`capture_spans`] sees the server task.

use std::net::SocketAddr;
use std::time::Duration;

use rust_auth_lib::ForwardedToken;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::{sleep, timeout};
use tonic::service::interceptor::InterceptedService;
use tonic::transport::Channel;
use tonic::{Code, Request, Response, Status};
use tonic_health::pb::health_client::HealthClient;
use tonic_health::pb::{HealthCheckRequest, health_check_response::ServingStatus};

use rust_grpc_lib::auth::{ClientJwtInterceptor, RequestContext};
use rust_grpc_lib::server::{AuthMode, GrpcServer, GrpcServerBuilder, ServeError};
use rust_grpc_lib::testing::{assert_all_rpcs_require_auth, capture_spans};

use crate::services::example::{
    IdList, ReplyOne, ReplyTwo,
    example_db_client::ExampleDbClient,
    example_db_server::{ExampleDb, ExampleDbServer, SERVICE_NAME},
};
use crate::support::{TEST_USER, make_jwt, make_validator, rpc_span};

/// `getEndPointOne` answers and reports who called; `getEndPointTwo` never
/// finishes in time, to exercise the timeout.
#[derive(Clone)]
struct Stub;

#[tonic::async_trait]
impl ExampleDb for Stub {
    async fn get_end_point_one(
        &self,
        request: Request<IdList>,
    ) -> Result<Response<ReplyOne>, Status> {
        RequestContext::<IdList>::try_from_request(request)?;
        Ok(Response::new(ReplyOne {
            section_one: vec![],
        }))
    }

    async fn get_end_point_two(
        &self,
        _request: Request<IdList>,
    ) -> Result<Response<ReplyTwo>, Status> {
        sleep(Duration::from_secs(30)).await;
        Err(Status::unimplemented("unreachable in the tests"))
    }
}

/// Like [`Stub`], but never reads claims: the unauthenticated opt-out has none.
#[derive(Clone)]
struct OpenStub;

#[tonic::async_trait]
impl ExampleDb for OpenStub {
    async fn get_end_point_one(
        &self,
        _request: Request<IdList>,
    ) -> Result<Response<ReplyOne>, Status> {
        Ok(Response::new(ReplyOne {
            section_one: vec![],
        }))
    }

    async fn get_end_point_two(
        &self,
        _request: Request<IdList>,
    ) -> Result<Response<ReplyTwo>, Status> {
        Err(Status::unimplemented("unused"))
    }
}

struct Running {
    addr: SocketAddr,
    shutdown: oneshot::Sender<()>,
    task: JoinHandle<Result<(), ServeError>>,
}

/// Bind a port, then serve `builder` on it until [`Running::shutdown`] fires.
async fn start<A: AuthMode + Send + 'static>(builder: GrpcServerBuilder<A>) -> Running {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (shutdown, rx) = oneshot::channel::<()>();
    let task = tokio::spawn(async move {
        builder
            .serve_on_listener(listener, async {
                let _ = rx.await;
            })
            .await
    });
    Running {
        addr,
        shutdown,
        task,
    }
}
async fn connect(addr: SocketAddr) -> Channel {
    Channel::from_shared(format!("http://{addr}"))
        .unwrap()
        .connect()
        .await
        .unwrap()
}

async fn health_status(addr: SocketAddr, service: &str) -> Result<ServingStatus, Status> {
    let response = HealthClient::new(connect(addr).await)
        .check(HealthCheckRequest {
            service: service.to_string(),
        })
        .await?;
    Ok(response.into_inner().status())
}

fn client(
    addr: SocketAddr,
    token: &str,
) -> ExampleDbClient<InterceptedService<Channel, ClientJwtInterceptor<ForwardedToken>>> {
    ExampleDbClient::from_endpoint_with_provider(
        &format!("http://{addr}"),
        ForwardedToken::new(token.to_string()),
    )
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn health_needs_no_token_and_reports_every_service() {
    let running = start(
        GrpcServer::builder()
            .authenticate(make_validator())
            .add_service(ExampleDbServer::new(Stub)),
    )
    .await;

    assert_eq!(
        health_status(running.addr, "").await.unwrap(),
        ServingStatus::Serving
    );
    assert_eq!(
        health_status(running.addr, SERVICE_NAME).await.unwrap(),
        ServingStatus::Serving
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn application_rpcs_require_a_valid_token() {
    let running = start(
        GrpcServer::builder()
            .authenticate(make_validator())
            .add_service(ExampleDbServer::new(Stub)),
    )
    .await;

    let err = client(running.addr, "")
        .get_end_point_one(IdList { id: vec![1] })
        .await
        .unwrap_err();
    assert_eq!(err.code(), Code::Unauthenticated);

    // The handler builds a `RequestContext`, which fails with `Internal` unless
    // the auth layer inserted claims, so success proves they arrived.
    client(running.addr, &make_jwt(&[]))
        .get_end_point_one(IdList { id: vec![1] })
        .await
        .expect("a valid token reaches the handler");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conformance_helper_accepts_the_builder() {
    let running = start(
        GrpcServer::builder()
            .authenticate(make_validator())
            .add_service(ExampleDbServer::new(Stub)),
    )
    .await;
    let channel = connect(running.addr).await;
    let paths = [
        format!("/{SERVICE_NAME}/getEndPointOne"),
        format!("/{SERVICE_NAME}/getEndPointTwo"),
    ];
    let paths: Vec<&str> = paths.iter().map(String::as_str).collect();

    // Health is not in the list: it is open by design.
    assert_all_rpcs_require_auth(channel, &paths).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn health_turns_not_serving_when_shutdown_begins() {
    let running = start(
        GrpcServer::builder()
            .authenticate(make_validator())
            .add_service(ExampleDbServer::new(Stub)),
    )
    .await;

    // An open Watch stream keeps receiving updates while the server drains.
    let mut watch = HealthClient::new(connect(running.addr).await)
        .watch(HealthCheckRequest {
            service: SERVICE_NAME.to_string(),
        })
        .await
        .unwrap()
        .into_inner();
    let first = watch.message().await.unwrap().unwrap();
    assert_eq!(first.status(), ServingStatus::Serving);

    running.shutdown.send(()).unwrap();

    let next = timeout(Duration::from_secs(5), watch.message())
        .await
        .expect("a status update must arrive when shutdown begins")
        .unwrap()
        .unwrap();
    assert_eq!(next.status(), ServingStatus::NotServing);

    // Graceful shutdown completes once the last stream is gone.
    drop(watch);
    let result = timeout(Duration::from_secs(5), running.task)
        .await
        .expect("the server must stop after shutdown")
        .unwrap();
    assert!(result.is_ok(), "{result:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn timeout_applies_to_application_rpcs() {
    let running = start(
        GrpcServer::builder()
            .timeout(Duration::from_millis(100))
            .authenticate(make_validator())
            .add_service(ExampleDbServer::new(Stub)),
    )
    .await;

    let err = client(running.addr, &make_jwt(&[]))
        .get_end_point_two(IdList { id: vec![1] })
        .await
        .unwrap_err();
    assert_eq!(err.code(), Code::Cancelled);
}

#[tokio::test]
async fn every_rpc_gets_a_span_including_rejections_and_health() {
    let (capture, _guard) = capture_spans();
    let running = start(
        GrpcServer::builder()
            .authenticate(make_validator())
            .add_service(ExampleDbServer::new(Stub)),
    )
    .await;

    client(running.addr, &make_jwt(&[]))
        .get_end_point_one(IdList { id: vec![1] })
        .await
        .unwrap();
    let rejected = client(running.addr, "")
        .get_end_point_two(IdList { id: vec![1] })
        .await
        .unwrap_err();
    assert_eq!(rejected.code(), Code::Unauthenticated);
    health_status(running.addr, "").await.unwrap();

    let ok = rpc_span(&capture, "getEndPointOne");
    assert_eq!(ok.field("grpc.status"), Some("Ok"));
    assert_eq!(ok.field("user"), Some(TEST_USER));
    assert_eq!(
        rpc_span(&capture, "getEndPointTwo").field("grpc.status"),
        Some("Unauthenticated"),
        "the trace layer is outermost, so it sees auth rejections"
    );
    assert_eq!(rpc_span(&capture, "Check").field("grpc.status"), Some("Ok"));
}

#[tokio::test]
async fn dangerously_unauthenticated_serves_without_a_token() {
    let running = start(
        GrpcServer::builder()
            .dangerously_unauthenticated()
            .add_service(ExampleDbServer::new(OpenStub)),
    )
    .await;

    client(running.addr, "")
        .get_end_point_one(IdList { id: vec![1] })
        .await
        .expect("no token is required in the opt-out mode");
    assert_eq!(
        health_status(running.addr, SERVICE_NAME).await.unwrap(),
        ServingStatus::Serving
    );
}

#[tokio::test]
async fn serving_on_an_occupied_port_is_a_bind_error() {
    let occupied = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = occupied.local_addr().unwrap();

    let err = GrpcServer::builder()
        .authenticate(make_validator())
        .add_service(ExampleDbServer::new(Stub))
        .serve(addr)
        .await
        .unwrap_err();

    assert!(matches!(err, ServeError::Bind(_)), "{err:?}");
}
