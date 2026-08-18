//! Full client→server gRPC round-trip integration tests.
//!
//! Each test spins up a real tonic server on a loopback TCP port, connects a
//! real tonic client, and exercises the complete auth path:
//!
//! ```text
//! gRPC client  →  Authorization: Bearer <JWT>
//! JwtValidationLayer  →  validates RS256 token, inserts KeycloakClaims
//! #[grpc_service] + #[roles(any("operator"))]  →  role check
//! handler  →  returns Ok or permission_denied
//! ```
//!
//! The service under test is `ExampleDB`, generated from the fixture file at
//! `src/fixtures/services.example.rs` (committed to the repository so that
//! `cargo test` requires no `build.rs` or live `protoc` invocation).
//!
//! To regenerate the fixtures after updating the `interface-definitions`
//! submodule, run `bash scripts/gen-test-fixtures.sh`.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

extern crate prost;

use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use rust_auth_lib::{ForwardedToken, test_fixtures};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::time::sleep;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::Server;
use tonic::{Code, Request, Response, Status};

use rust_grpc_lib::auth::{StaticKeysValidator, StaticKeysValidatorConfig, validator_into_layer};

use crate::services::example::{
    IdList, ReplyOne, ReplyTwo,
    example_db_client::ExampleDbClient,
    example_db_server::{ExampleDb, ExampleDbServer},
};

const TEST_KID: &str = "integration-test-kid";

// ---------------------------------------------------------------------------
// JWT helpers
// ---------------------------------------------------------------------------

fn make_jwt(realm_roles: &[&str]) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let roles_json: Vec<serde_json::Value> =
        realm_roles.iter().map(|r| serde_json::json!(r)).collect();

    let payload = serde_json::json!({
        "sub": "test-user",
        "iat": now,
        "exp": now + 3600,
        "iss": null,
        "preferred_username": "tester",
        "realm_access": { "roles": roles_json }
    });

    let encoding_key = EncodingKey::from_rsa_pem(test_fixtures::RSA_PRIVATE_PEM.as_bytes())
        .expect("valid RSA PEM");
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(TEST_KID.to_string());
    encode(&header, &payload, &encoding_key).expect("token encoding must succeed")
}

fn make_validator() -> Arc<StaticKeysValidator> {
    let jwks = test_fixtures::make_jwks_json_str(TEST_KID);
    let config = StaticKeysValidatorConfig::from_jwks_str(&jwks);
    Arc::new(StaticKeysValidator::new(config).expect("validator construction must succeed"))
}

// ---------------------------------------------------------------------------
// Stub ExampleDB server implementation
//
// We implement the generated `ExampleDb` trait on a unit struct. Only
// `get_end_point_one` is gated with `#[roles(any("operator"))]`; the other
// method returns `unimplemented` — it is never called by these tests.
// ---------------------------------------------------------------------------

struct StubExampleDb;

#[rust_grpc_lib::grpc_service]
#[tonic::async_trait]
impl ExampleDb for StubExampleDb {
    #[roles(any("operator"))]
    async fn get_end_point_one(
        &self,
        request: Request<IdList>,
    ) -> Result<Response<ReplyOne>, Status> {
        // Echo the id list back as an empty reply so the test can verify
        // the response was actually produced by this handler.
        let _ids = request.into_inner().id;
        Ok(Response::new(ReplyOne {
            section_one: vec![],
        }))
    }

    async fn get_end_point_two(
        &self,
        _request: Request<IdList>,
    ) -> Result<Response<ReplyTwo>, Status> {
        Err(Status::unimplemented("not used in integration tests"))
    }
}

// ---------------------------------------------------------------------------
// Test fixture: spin up a server and return the bound address
// ---------------------------------------------------------------------------

async fn start_server() -> (SocketAddr, oneshot::Sender<()>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind to loopback:0 must succeed");
    let addr = listener.local_addr().expect("must have local addr");

    let validator = make_validator();
    let auth_layer = validator_into_layer(validator);

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

    tokio::spawn(async move {
        let incoming = TcpListenerStream::new(listener);

        Server::builder()
            .layer(auth_layer)
            .add_service(ExampleDbServer::new(StubExampleDb))
            .serve_with_incoming_shutdown(incoming, async {
                let _ = shutdown_rx.await;
            })
            .await
            .expect("server must not error");
    });

    // Give the server a moment to start accepting connections.
    sleep(Duration::from_millis(50)).await;

    (addr, shutdown_tx)
}

// ---------------------------------------------------------------------------
// Test 1: valid operator JWT → RPC succeeds
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_round_trip_with_valid_operator_jwt_succeeds() {
    let (addr, _shutdown) = start_server().await;
    let endpoint = format!("http://{addr}");

    let operator_jwt = make_jwt(&["operator"]);
    let mut client =
        ExampleDbClient::from_endpoint_with_provider(&endpoint, ForwardedToken::new(operator_jwt))
            .unwrap();

    let response = client
        .get_end_point_one(IdList { id: vec![1, 2, 3] })
        .await
        .expect("RPC must succeed when caller has the 'operator' role");

    // The stub returns an empty set — just verify we got a response.
    assert!(
        response.into_inner().section_one.is_empty(),
        "stub handler must return an empty ReplyOne"
    );
}

// ---------------------------------------------------------------------------
// Test 2: JWT with wrong role → permission_denied
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_round_trip_with_wrong_role_returns_permission_denied() {
    let (addr, _shutdown) = start_server().await;
    let endpoint = format!("http://{addr}");

    // "viewer" is not "operator" — the role check must reject this.
    let viewer_jwt = make_jwt(&["viewer"]);
    let mut client =
        ExampleDbClient::from_endpoint_with_provider(&endpoint, ForwardedToken::new(viewer_jwt))
            .unwrap();

    let err = client
        .get_end_point_one(IdList { id: vec![1] })
        .await
        .unwrap_err();

    assert_eq!(
        err.code(),
        Code::PermissionDenied,
        "status must be PERMISSION_DENIED when the required role is absent"
    );
}

// ---------------------------------------------------------------------------
// Test 3: no Authorization header → unauthenticated
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_round_trip_with_no_token_returns_unauthenticated() {
    let (addr, _shutdown) = start_server().await;
    let endpoint = format!("http://{addr}");

    // No token — JwtValidationLayer must reject the request before it reaches
    // the handler.
    let mut client =
        ExampleDbClient::from_endpoint_with_provider(&endpoint, ForwardedToken::new(String::new()))
            .unwrap();

    let err = client
        .get_end_point_one(IdList { id: vec![1] })
        .await
        .unwrap_err();

    assert_eq!(
        err.code(),
        Code::Unauthenticated,
        "status must be UNAUTHENTICATED when the Authorization header is missing"
    );
}

// ---------------------------------------------------------------------------
// Test 4: no Authorization header → unauthenticated with "unauthenticated" feature
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_round_trip_with_no_auth_header_returns_unauthenticated() {
    let (addr, _shutdown) = start_server().await;
    let endpoint = format!("http://{addr}");

    // No token — JwtValidationLayer must reject the request before it reaches
    // the handler.
    let mut client = ExampleDbClient::from_endpoint(&endpoint).unwrap();

    let err = client
        .get_end_point_one(IdList { id: vec![1] })
        .await
        .unwrap_err();

    assert_eq!(
        err.code(),
        Code::Unauthenticated,
        "status must be UNAUTHENTICATED when the Authorization header is missing"
    );
}
