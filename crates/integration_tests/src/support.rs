//! Helpers shared by the live-server tests: token and validator factories,
//! plus an `rpc` span lookup on top of `rust_grpc_lib::testing`'s span capture.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use rust_auth_lib::test_fixtures;

use rust_grpc_lib::auth::{KeyValidator, KeyValidatorConfig};
use rust_grpc_lib::testing::{SpanCapture, SpanRecord};

/// The `kid` of the test signing key, shared by [`make_jwt`] and
/// [`make_validator`].
pub const TEST_KID: &str = "integration-test-kid";

/// The `sub` claim of every token from [`make_jwt`].
pub const TEST_USER: &str = "test-user";

/// Mint an RS256 token for [`TEST_USER`] holding the given realm roles, valid
/// for an hour.
pub fn make_jwt(roles: &[&str]) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let payload = serde_json::json!({
        "sub": TEST_USER,
        "iat": now,
        "exp": now + 3600,
        "realm_access": { "roles": roles },
    });
    let key = EncodingKey::from_rsa_pem(test_fixtures::RSA_PRIVATE_PEM.as_bytes()).unwrap();
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(TEST_KID.to_string());
    encode(&header, &payload, &key).unwrap()
}

/// A validator that accepts the tokens minted by [`make_jwt`].
pub fn make_validator() -> Arc<KeyValidator> {
    let jwks = test_fixtures::make_jwks_json_str(TEST_KID);
    Arc::new(KeyValidator::new(KeyValidatorConfig::from_jwks_str(&jwks)).unwrap())
}

/// The single `rpc` span (the one `GrpcTraceLayer` opens) whose `rpc.method`
/// is `method`.
pub fn rpc_span(capture: &SpanCapture, method: &str) -> SpanRecord {
    let mut matching: Vec<_> = capture
        .spans("rpc")
        .into_iter()
        .filter(|span| span.field("rpc.method") == Some(method))
        .collect();
    assert_eq!(matching.len(), 1, "expected one rpc span for {method}");
    matching.remove(0)
}
