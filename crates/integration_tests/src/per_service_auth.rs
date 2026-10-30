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
//! - `ExampleDbServer::with_interceptor(inner, JwtValidationService::new(..))`.
//!
//! The `test-util` conformance helpers (`assert_all_rpcs_require_auth` and
//! `assert_all_rpcs_require_role`) are run against the same wiring, plus
//! against a server that forgot to install auth, which they must flag.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
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
    ClientJwtInterceptor, JwtValidationService, KeyValidator, KeyValidatorConfig, KeycloakClaims,
    RequestContext, wrap_service,
};
use rust_grpc_lib::testing::{assert_all_rpcs_require_auth, assert_all_rpcs_require_role};

use crate::services::example::{
    IdList, ReplyOne, ReplyTwo,
    example_db_client::ExampleDbClient,
    example_db_server::{ExampleDb, ExampleDbServer, SERVICE_NAME},
};

const TEST_KID: &str = "per-service-test-kid";

fn make_jwt(roles: &[&str]) -> String {
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
    /// No auth at all: the mistake the conformance helpers exist to catch.
    Unprotected,
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
            JwtValidationService::<KeycloakClaims, _>::new(validator),
        )),
        Wiring::Unprotected => router.add_service(ExampleDbServer::new(Stub)),
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

// ---------------------------------------------------------------------------
// Conformance helpers
// ---------------------------------------------------------------------------

fn rpc_paths() -> [String; 2] {
    [
        format!("/{SERVICE_NAME}/getEndPointOne"),
        format!("/{SERVICE_NAME}/getEndPointTwo"),
    ]
}

async fn connect(addr: SocketAddr) -> Channel {
    Channel::from_shared(format!("http://{addr}"))
        .unwrap()
        .connect()
        .await
        .unwrap()
}

async fn assert_conformance(wiring: Wiring) {
    let running = start(wiring).await;
    let channel = connect(running.addr).await;
    let paths = rpc_paths();
    let paths: Vec<&str> = paths.iter().map(String::as_str).collect();

    assert_all_rpcs_require_auth(channel.clone(), &paths).await;

    // Only `getEndPointOne` declares a role (the stub checks for "operator").
    assert_all_rpcs_require_role(channel, &paths[..1], &make_jwt(&["viewer"])).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conformance_helpers_pass_with_wrap_service() {
    assert_conformance(Wiring::WrapService).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conformance_helpers_pass_with_with_interceptor() {
    assert_conformance(Wiring::WithInterceptor).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[should_panic(expected = "auth conformance failed")]
async fn conformance_flags_a_server_without_auth() {
    let running = start(Wiring::Unprotected).await;
    let channel = connect(running.addr).await;
    let paths = rpc_paths();
    let paths: Vec<&str> = paths.iter().map(String::as_str).collect();

    assert_all_rpcs_require_auth(channel, &paths).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[should_panic(expected = "expected PermissionDenied")]
async fn role_conformance_flags_a_token_that_is_allowed() {
    let running = start(Wiring::WrapService).await;
    let channel = connect(running.addr).await;
    let path = format!("/{SERVICE_NAME}/getEndPointOne");

    // An operator token is allowed, so the "without the role" check must fail.
    assert_all_rpcs_require_role(channel, &[path.as_str()], &make_jwt(&["operator"])).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[should_panic(expected = "open service")]
async fn conformance_refuses_the_health_service() {
    let running = start(Wiring::WrapService).await;
    let channel = connect(running.addr).await;

    assert_all_rpcs_require_auth(channel, &["/grpc.health.v1.Health/Check"]).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_rpc_reports_the_status_for_a_valid_token() {
    let running = start(Wiring::WrapService).await;
    let channel = connect(running.addr).await;
    let path = format!("/{SERVICE_NAME}/getEndPointOne");

    let ok =
        rust_grpc_lib::testing::call_rpc(channel.clone(), &path, Some(&make_jwt(&["operator"])))
            .await;
    assert!(ok.is_ok(), "{ok:?}");

    let denied = rust_grpc_lib::testing::call_rpc(channel, &path, None)
        .await
        .unwrap_err();
    assert_eq!(denied.code(), Code::Unauthenticated);
}
