//! `GrpcTraceLayer` tests against a live in-process server.
//!
//! The server is wired the way the docs recommend: the trace layer is the
//! first (outermost) `Server::layer`, a `tonic-health` service is added
//! unwrapped, and the application service is wrapped with per-service auth.
//!
//! Every test runs on the default current-thread runtime, so the server tasks
//! and the client run on the test's own thread and the thread-local
//! subscriber installed by [`capture_spans`] sees all of them.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rust_auth_lib::ForwardedToken;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::time::sleep;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::service::interceptor::InterceptedService;
use tonic::transport::{Channel, Server};
use tonic::{Code, Request, Response, Status};
use tonic_health::pb::HealthCheckRequest;
use tonic_health::pb::health_client::HealthClient;

use rust_grpc_lib::auth::{
    ClientJwtInterceptor, KeycloakClaims, RequestContext, RpcMethod, wrap_service,
};
use rust_grpc_lib::testing::capture_spans;
use rust_grpc_lib::trace::GrpcTraceLayer;

use crate::services::example::{
    IdList, ReplyOne, ReplyTwo,
    example_db_client::ExampleDbClient,
    example_db_server::{ExampleDb, ExampleDbServer, SERVICE_NAME},
};
use crate::support::{TEST_USER, make_jwt, make_validator, rpc_span};

// ---------------------------------------------------------------------------
// Server under test
// ---------------------------------------------------------------------------

/// Reads the claims through `RequestContext`, from inside its own
/// `#[instrument]` span, and remembers the `rpc_method` it saw.
struct Stub {
    seen_method: Arc<Mutex<Option<RpcMethod>>>,
}

#[tonic::async_trait]
impl ExampleDb for Stub {
    #[tracing::instrument(skip_all)]
    async fn get_end_point_one(
        &self,
        request: Request<IdList>,
    ) -> Result<Response<ReplyOne>, Status> {
        let ctx = RequestContext::<IdList>::try_from_request(request)?;
        *self.seen_method.lock().unwrap() = ctx.rpc_method.clone();
        Ok(Response::new(ReplyOne {
            section_one: vec![],
        }))
    }

    /// Never finishes in time: used to trigger the server's timeout.
    async fn get_end_point_two(
        &self,
        _request: Request<IdList>,
    ) -> Result<Response<ReplyTwo>, Status> {
        sleep(Duration::from_secs(30)).await;
        Err(Status::unimplemented("unreachable in the tests"))
    }
}

struct Running {
    addr: SocketAddr,
    seen_method: Arc<Mutex<Option<RpcMethod>>>,
    _shutdown: oneshot::Sender<()>,
}

async fn start(timeout: Option<Duration>) -> Running {
    // The listener is bound before the server task is spawned, so connections
    // that arrive early wait in the accept backlog; no startup delay needed.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen_method = Arc::new(Mutex::new(None));

    let (reporter, health) = tonic_health::server::health_reporter();
    reporter
        .set_service_status("", tonic_health::ServingStatus::Serving)
        .await;

    let mut builder = Server::builder();
    if let Some(timeout) = timeout {
        builder = builder.timeout(timeout);
    }
    // The trace layer goes first so it is outermost; auth is per service.
    let router = builder
        .layer(GrpcTraceLayer::new())
        .add_service(health)
        .add_service(wrap_service::<KeycloakClaims, _, _>(
            ExampleDbServer::new(Stub {
                seen_method: Arc::clone(&seen_method),
            }),
            make_validator(),
        ));

    let (tx, rx) = oneshot::channel::<()>();
    tokio::spawn(async move {
        router
            .serve_with_incoming_shutdown(TcpListenerStream::new(listener), async {
                let _ = rx.await;
            })
            .await
            .unwrap();
    });

    Running {
        addr,
        seen_method,
        _shutdown: tx,
    }
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unary_rpc_gets_a_span_with_fields_and_final_status() {
    let (capture, _guard) = capture_spans();
    let running = start(None).await;

    client(running.addr, &make_jwt(&[]))
        .get_end_point_one(IdList { id: vec![1] })
        .await
        .expect("a valid token reaches the handler");

    let span = rpc_span(&capture, "getEndPointOne");
    assert_eq!(span.field("rpc.system"), Some("grpc"));
    assert_eq!(span.field("rpc.service"), Some(SERVICE_NAME));
    assert_eq!(
        span.field("grpc.status"),
        Some("Ok"),
        "the status arrives in the trailers and is recorded at end of stream"
    );
    assert!(span.has_field("latency_ms"));
    assert!(span.has_field("stream_ms"));
}

#[tokio::test]
async fn handler_sees_the_rpc_method_and_user_lands_on_the_rpc_span_despite_instrument() {
    let (capture, _guard) = capture_spans();
    let running = start(None).await;

    client(running.addr, &make_jwt(&[]))
        .get_end_point_one(IdList { id: vec![1] })
        .await
        .unwrap();

    assert_eq!(
        *running.seen_method.lock().unwrap(),
        Some(RpcMethod {
            service: SERVICE_NAME.to_string(),
            method: "getEndPointOne".to_string(),
        })
    );

    let rpc = rpc_span(&capture, "getEndPointOne");
    assert_eq!(rpc.field("user"), Some(TEST_USER));

    // The handler's own span is the innermost span while `try_from_request`
    // runs. It does not declare `user`, so `Span::current().record("user", ..)`
    // would have been lost.
    let handler_spans = capture.spans("get_end_point_one");
    assert_eq!(handler_spans.len(), 1, "the handler's #[instrument] span");
    assert!(!handler_spans[0].has_field("user"));
}

#[tokio::test]
async fn auth_rejection_is_traced_with_its_status_and_no_user() {
    let (capture, _guard) = capture_spans();
    let running = start(None).await;

    let err = client(running.addr, "")
        .get_end_point_one(IdList { id: vec![1] })
        .await
        .unwrap_err();
    assert_eq!(err.code(), Code::Unauthenticated);

    let span = rpc_span(&capture, "getEndPointOne");
    assert_eq!(
        span.field("grpc.status"),
        Some("Unauthenticated"),
        "the layer sits outside auth, so it sees the rejection"
    );
    assert!(!span.has_field("user"));
    assert!(
        capture.spans("get_end_point_one").is_empty(),
        "the handler must not have run"
    );
}

#[tokio::test]
async fn health_checks_are_traced_without_a_token() {
    let (capture, _guard) = capture_spans();
    let running = start(None).await;

    let channel = Channel::from_shared(format!("http://{}", running.addr))
        .unwrap()
        .connect()
        .await
        .unwrap();
    HealthClient::new(channel)
        .check(HealthCheckRequest {
            service: String::new(),
        })
        .await
        .expect("health needs no token");

    let span = rpc_span(&capture, "Check");
    assert_eq!(span.field("rpc.service"), Some("grpc.health.v1.Health"));
    assert_eq!(span.field("grpc.status"), Some("Ok"));
    assert!(!span.has_field("user"));
}

#[tokio::test]
async fn timed_out_rpc_is_recorded_as_cancelled() {
    let (capture, _guard) = capture_spans();
    let running = start(Some(Duration::from_millis(100))).await;

    let err = client(running.addr, &make_jwt(&[]))
        .get_end_point_two(IdList { id: vec![1] })
        .await
        .unwrap_err();
    assert_eq!(err.code(), Code::Cancelled);

    // tonic's timeout drops the handler future, so no response was ever
    // produced for the trace layer to classify.
    let span = rpc_span(&capture, "getEndPointTwo");
    assert_eq!(span.field("grpc.status"), Some("Cancelled"));
}
