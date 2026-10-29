//! Per-service JWT auth tests.
//!
//! The server adds a `tonic-health` service **unwrapped** and the application
//! service wrapped with the library's public auth constructors, with no
//! global `Server::layer`. This is the wiring recommended for services that
//! must keep health probes open.
//!
//! Both public routes are covered:
//!
//! - [`wrap_service`] (no direct `tower` dependency needed), and
//! - `ExampleDbServer::with_interceptor(inner, validator_into_interceptor(..))`.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use rust_auth_lib::{ForwardedToken, test_fixtures};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::time::sleep;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::service::interceptor::InterceptedService;
use tonic::transport::{Channel, Server};
use tonic::{Code, Request, Response, Status};
use tonic_health::pb::health_client::HealthClient;
use tonic_health::pb::{HealthCheckRequest, health_check_response::ServingStatus};

use rust_grpc_lib::auth::{
    ClientJwtInterceptor, KeyValidator, KeyValidatorConfig, KeycloakClaims, RequestContext,
    validator_into_interceptor, wrap_service,
};

use crate::services::example::{
    IdList, ReplyOne, ReplyTwo,
    example_db_client::ExampleDbClient,
    example_db_server::{ExampleDb, ExampleDbServer, SERVICE_NAME},
};

const TEST_KID: &str = "per-service-test-kid";

fn make_jwt(roles: &[&str]) -> String {
    use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
    use std::time::{SystemTime, UNIX_EPOCH};

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let payload = serde_json::json!({
        "sub": "test-user",
        "iat": now,
        "exp": now + 3600,
        "realm_access": { "roles": roles },
    });
    let key = EncodingKey::from_rsa_pem(test_fixtures::RSA_PRIVATE_PEM.as_bytes()).unwrap();
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(TEST_KID.to_string());
    encode(&header, &payload, &key).unwrap()
}

fn make_validator() -> Arc<KeyValidator> {
    let jwks = test_fixtures::make_jwks_json_str(TEST_KID);
    Arc::new(KeyValidator::new(KeyValidatorConfig::from_jwks_str(&jwks)).unwrap())
}

/// A handler that reads the claims through `RequestContext`.
struct Stub;

#[tonic::async_trait]
impl ExampleDb for Stub {
    async fn get_end_point_one(
        &self,
        request: Request<IdList>,
    ) -> Result<Response<ReplyOne>, Status> {
        let ctx = RequestContext::<IdList>::try_from_request(request)?;
        if !ctx.has_role("operator") {
            return Err(Status::permission_denied("operator role required"));
        }
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

#[derive(Clone, Copy)]
enum Wiring {
    WrapService,
    WithInterceptor,
}

struct Running {
    addr: SocketAddr,
    reporter: tonic_health::server::HealthReporter,
    _shutdown: oneshot::Sender<()>,
}

async fn start(wiring: Wiring) -> Running {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let validator = make_validator();

    let (reporter, health) = tonic_health::server::health_reporter();
    // `set_serving` must keep working on the generated (inner) server type.
    reporter
        .set_service_status("", tonic_health::ServingStatus::Serving)
        .await;
    reporter.set_serving::<ExampleDbServer<Stub>>().await;

    let (tx, rx) = oneshot::channel::<()>();
    let incoming = TcpListenerStream::new(listener);

    // No `Server::layer`: auth is applied per service, health stays open.
    let router = Server::builder().add_service(health);
    let router = match wiring {
        Wiring::WrapService => router.add_service(wrap_service::<KeycloakClaims, _, _>(
            ExampleDbServer::new(Stub),
            validator,
        )),
        Wiring::WithInterceptor => router.add_service(ExampleDbServer::with_interceptor(
            Stub,
            validator_into_interceptor::<KeycloakClaims, _>(validator),
        )),
    };

    tokio::spawn(async move {
        router
            .serve_with_incoming_shutdown(incoming, async {
                let _ = rx.await;
            })
            .await
            .unwrap();
    });
    sleep(Duration::from_millis(50)).await;

    Running {
        addr,
        reporter,
        _shutdown: tx,
    }
}

async fn health_status(addr: SocketAddr, service: &str) -> Result<ServingStatus, Status> {
    let channel = Channel::from_shared(format!("http://{addr}"))
        .unwrap()
        .connect()
        .await
        .unwrap();
    let response = HealthClient::new(channel)
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

async fn assert_behaviour(wiring: Wiring) {
    let running = start(wiring).await;
    let addr = running.addr;

    // Health needs no token, for both "" and the service name.
    assert_eq!(
        health_status(addr, "").await.unwrap(),
        ServingStatus::Serving
    );
    assert_eq!(
        health_status(addr, SERVICE_NAME).await.unwrap(),
        ServingStatus::Serving,
        "set_serving::<ExampleDbServer<_>>() must work with per-service auth"
    );

    // An application RPC without a token is rejected.
    let err = client(addr, "")
        .get_end_point_one(IdList { id: vec![1] })
        .await
        .unwrap_err();
    assert_eq!(err.code(), Code::Unauthenticated);

    // A valid token without the role reaches the handler, which rejects it.
    let err = client(addr, &make_jwt(&["viewer"]))
        .get_end_point_one(IdList { id: vec![1] })
        .await
        .unwrap_err();
    assert_eq!(err.code(), Code::PermissionDenied);

    // A valid token with the role reaches the handler and succeeds.
    client(addr, &make_jwt(&["operator"]))
        .get_end_point_one(IdList { id: vec![1] })
        .await
        .expect("operator token must reach the handler");

    // Health stays reachable, and still tracks status changes.
    let reporter = running.reporter.clone();
    reporter.set_not_serving::<ExampleDbServer<Stub>>().await;
    assert_eq!(
        health_status(addr, SERVICE_NAME).await.unwrap(),
        ServingStatus::NotServing
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wrap_service_guards_app_rpcs_and_leaves_health_open() {
    assert_behaviour(Wiring::WrapService).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn with_interceptor_guards_app_rpcs_and_leaves_health_open() {
    assert_behaviour(Wiring::WithInterceptor).await;
}
