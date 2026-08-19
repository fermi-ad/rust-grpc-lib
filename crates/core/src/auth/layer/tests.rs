//! Tests for `JwtValidationService` — covers missing headers, invalid tokens,
//! and successful validation with claims insertion.

use super::*;

use std::sync::Arc;

use rust_auth_lib::{Claims, StaticKeysValidator, StaticKeysValidatorConfig, test_fixtures};
use tonic::Code;

/// Build a `JwtValidationService` backed by an in-memory JWKS.
fn make_service(kid: &str) -> JwtValidationService<StaticKeysValidator> {
    let jwks = test_fixtures::make_jwks_json_str(kid);
    let config = StaticKeysValidatorConfig::from_jwks_str(&jwks);
    let validator = StaticKeysValidator::new(config).expect("validator construction must succeed");
    JwtValidationService::new(Arc::new(validator))
}

// -----------------------------------------------------------------------
// Test a: missing Authorization header → unauthenticated
// -----------------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_auth_header_returns_unauthenticated() {
    let mut svc = make_service("test-kid");

    // No Authorization metadata entry.
    let req = Request::new(());
    let result = svc.call(req);

    let err = result.expect_err("call must fail when Authorization header is absent");
    assert_eq!(
        err.code(),
        Code::Unauthenticated,
        "status code must be UNAUTHENTICATED"
    );
}

// -----------------------------------------------------------------------
// Test b: invalid token → unauthenticated
// -----------------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_token_returns_unauthenticated() {
    let mut svc = make_service("test-kid");

    let mut req = Request::new(());
    req.metadata_mut()
        .insert("authorization", "Bearer not-a-valid-jwt".parse().unwrap());

    let result = svc.call(req);

    let err = result.expect_err("call must fail for an invalid JWT");
    assert_eq!(
        err.code(),
        Code::Unauthenticated,
        "status code must be UNAUTHENTICATED for a bad token"
    );
}

// -----------------------------------------------------------------------
// Test c: valid token → KeycloakClaims inserted into extensions
// -----------------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn valid_token_inserts_claims_into_extensions() {
    const KID: &str = "test-kid";
    let mut svc = make_service(KID);

    let token = test_fixtures::make_token(Some(KID), 300, test_fixtures::RSA_PRIVATE_PEM);
    let bearer = format!("Bearer {token}");

    let mut req = Request::new(());
    req.metadata_mut()
        .insert("authorization", bearer.parse().unwrap());

    let enriched = svc.call(req).expect("call must succeed for a valid JWT");

    let claims = enriched
        .extensions()
        .get::<KeycloakClaims>()
        .expect("KeycloakClaims must be present in request extensions after successful validation");

    assert_eq!(
        claims.sub(),
        "test-sub",
        "claims subject must match the fixture token's sub"
    );
    assert!(
        !claims.has_role("superuser"),
        "claims must not carry a role that was not in the token"
    );
}
