//! Full client→server gRPC round-trip integration tests.
//!
//! These tests spin up a real tonic server on a loopback TCP port, connect a
//! real tonic client, and exercise the complete auth path:
//!
//!   gRPC client  →  Authorization: Bearer <JWT>
//!   JwtValidationLayer  →  validates RS256 token, inserts KeycloakClaims
//!   #[grpc_service] + #[roles(any("operator"))]  →  role check
//!   handler  →  returns Ok or permission_denied
//!
//! The service used is the real `DevDB` gRPC service generated from the
//! bundled `DevDB.proto` definition. The generated Rust source is committed
//! to `tests/fixtures/` so no build.rs or live protoc invocation is needed
//! at test time.
//!
//! If the `interface-definitions` submodule is updated, re-run:
//!   bash scripts/gen-test-fixtures.sh
//! and commit the regenerated files.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

extern crate prost;

use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use rust_auth_lib::test_fixtures;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::time::sleep;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::service::Interceptor;
use tonic::service::interceptor::InterceptedService;
use tonic::transport::{Channel, Server};
use tonic::{Code, Request, Response, Status};

use rust_grpc_lib::auth::layer::new_jwt_validation_layer;
use rust_grpc_lib::auth::{StaticKeysValidator, StaticKeysValidatorConfig};

// ---------------------------------------------------------------------------
// Include the pre-generated proto fixtures.
//
// The generated `services.devdb` module references
// `super::super::super::google::protobuf::Empty`, so we must nest it inside
// `services::devdb` and expose `google::protobuf` at the crate root level.
// ---------------------------------------------------------------------------

pub mod google {
    pub mod protobuf {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/google.protobuf.rs"
        ));
    }
}

pub mod services {
    pub mod devdb {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/services.devdb.rs"
        ));
    }
}

use services::devdb::{
    DeviceInfoReply, DeviceList,
    dev_db_client::DevDbClient,
    dev_db_server::{DevDb, DevDbServer},
};

use crate::google::protobuf::Empty;
use crate::services::devdb::{
    AlarmInfoReply, AlarmTextIdList, DeviceAlarmTextList, PlotConfig, PlotConfigResult,
    PlotConfigSpecification, PlotSelector,
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
// Stub DevDB server implementation
//
// We implement the generated `DevDb` trait on a unit struct. Only
// `get_device_info` is gated with `#[roles(any("operator"))]`; the other
// methods return `unimplemented` — they are never called by these tests.
// ---------------------------------------------------------------------------

struct StubDevDb;

#[rust_grpc_lib::grpc_service]
#[tonic::async_trait]
impl DevDb for StubDevDb {
    #[roles(any("operator"))]
    async fn get_device_info(
        &self,
        request: Request<DeviceList>,
    ) -> Result<Response<DeviceInfoReply>, Status> {
        // Echo the device names back in the reply so the test can verify
        // the response was actually produced by this handler.
        let _devices = request.into_inner().device;
        Ok(Response::new(DeviceInfoReply { set: vec![] }))
    }

    async fn get_all_alarm_info(
        &self,
        _request: Request<DeviceList>,
    ) -> Result<Response<AlarmInfoReply>, Status> {
        Err(Status::unimplemented("not used in integration tests"))
    }

    async fn get_alarm_text(
        &self,
        _request: Request<AlarmTextIdList>,
    ) -> Result<Response<DeviceAlarmTextList>, Status> {
        Err(Status::unimplemented("not used in integration tests"))
    }

    async fn get_plot_configuration(
        &self,
        _request: Request<PlotSelector>,
    ) -> Result<Response<PlotConfigResult>, Status> {
        Err(Status::unimplemented("not used in integration tests"))
    }

    async fn get_user_plot_configuration(
        &self,
        _request: Request<Empty>,
    ) -> Result<Response<PlotConfigResult>, Status> {
        Err(Status::unimplemented("not used in integration tests"))
    }

    async fn delete_plot_configuration(
        &self,
        _request: Request<PlotSelector>,
    ) -> Result<Response<PlotConfigResult>, Status> {
        Err(Status::unimplemented("not used in integration tests"))
    }

    async fn save_plot_configuration(
        &self,
        _request: Request<PlotConfigSpecification>,
    ) -> Result<Response<PlotConfigResult>, Status> {
        Err(Status::unimplemented("not used in integration tests"))
    }

    async fn save_user_plot_configuration(
        &self,
        _request: Request<PlotConfig>,
    ) -> Result<Response<PlotConfigResult>, Status> {
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
    let auth_layer = new_jwt_validation_layer(validator);

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

    tokio::spawn(async move {
        let incoming = TcpListenerStream::new(listener);

        Server::builder()
            .layer(auth_layer)
            .add_service(DevDbServer::new(StubDevDb))
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

/// Build a `DevDbClient` that optionally attaches a Bearer token.
async fn make_client(
    addr: SocketAddr,
    bearer_token: Option<String>,
) -> DevDbClient<InterceptedService<Channel, impl Interceptor>> {
    let endpoint = format!("http://{addr}");
    let channel = Channel::from_shared(endpoint)
        .expect("valid endpoint URI")
        .connect()
        .await
        .expect("channel connect must succeed");

    let token = bearer_token.unwrap_or_default();
    DevDbClient::with_interceptor(channel, move |mut req: Request<()>| {
        if !token.is_empty() {
            let bearer = format!("Bearer {token}");
            req.metadata_mut()
                .insert("authorization", bearer.parse().unwrap());
        }
        Ok(req)
    })
}

// ---------------------------------------------------------------------------
// Test 1: valid operator JWT → RPC succeeds
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_round_trip_with_valid_operator_jwt_succeeds() {
    let (addr, _shutdown) = start_server().await;

    let operator_jwt = make_jwt(&["operator"]);
    let mut client = make_client(addr, Some(operator_jwt)).await;

    let response = client
        .get_device_info(DeviceList {
            device: vec!["M:OUTTMP".to_string()],
        })
        .await
        .expect("RPC must succeed when caller has the 'operator' role");

    // The stub returns an empty set — just verify we got a response.
    assert!(
        response.into_inner().set.is_empty(),
        "stub handler must return an empty DeviceInfoReply"
    );
}

// ---------------------------------------------------------------------------
// Test 2: JWT with wrong role → permission_denied
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_round_trip_with_wrong_role_returns_permission_denied() {
    let (addr, _shutdown) = start_server().await;

    // "viewer" is not "operator" — the role check must reject this.
    let viewer_jwt = make_jwt(&["viewer"]);
    let mut client = make_client(addr, Some(viewer_jwt)).await;

    let err = client
        .get_device_info(DeviceList {
            device: vec!["M:OUTTMP".to_string()],
        })
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

    // No token — JwtValidationLayer must reject the request before it reaches
    // the handler.
    let mut client = make_client(addr, None).await;

    let err = client
        .get_device_info(DeviceList {
            device: vec!["M:OUTTMP".to_string()],
        })
        .await
        .unwrap_err();

    assert_eq!(
        err.code(),
        Code::Unauthenticated,
        "status must be UNAUTHENTICATED when the Authorization header is missing"
    );
}
