//! Tests for [`super::JwtValidationService`].
//!
//! Covers: missing `Authorization` header, invalid JWT, and a valid JWT that
//! results in [`super::KeycloakClaims`] being inserted into request extensions.

use super::*;

use std::sync::Arc;

use rust_auth_lib::{
    Claims, KeyValidator, KeyValidatorConfig, keycloak::KeycloakClaims, test_fixtures,
};
use tonic::{Code, server::NamedService};

/// Minimal service used to check that `wrap_service` keeps `NamedService`.
#[derive(Clone)]
struct Named;

impl NamedService for Named {
    const NAME: &'static str = "pkg.Named";
}

/// Build an in-memory-JWKS validator.
fn make_validator(kid: &str) -> Arc<KeyValidator> {
    let jwks = test_fixtures::make_jwks_json_str(kid);
    let config = KeyValidatorConfig::from_jwks_str(&jwks);
    Arc::new(KeyValidator::new(config).expect("validator construction must succeed"))
}

/// Build a `JwtValidationService` backed by an in-memory JWKS.
fn make_service(kid: &str) -> JwtValidationService<KeycloakClaims, KeyValidator> {
    JwtValidationService::new(make_validator(kid))
}

/// A request carrying a valid bearer token signed for `kid`.
fn bearer_request(kid: &str) -> Request<()> {
    let token = test_fixtures::make_token(Some(kid), 300, test_fixtures::RSA_PRIVATE_PEM);
    let mut req = Request::new(());
    req.metadata_mut()
        .insert("authorization", format!("Bearer {token}").parse().unwrap());
    req
}

fn name_of<S: NamedService>(_: &S) -> &'static str {
    S::NAME
}

// -----------------------------------------------------------------------
// Public constructor: new, clone
// -----------------------------------------------------------------------
#[test]
fn new_builds_a_working_interceptor() {
    const KID: &str = "ctor-kid";
    let mut svc = JwtValidationService::<KeycloakClaims, _>::new(make_validator(KID));

    let enriched = svc
        .call(bearer_request(KID))
        .expect("valid token must pass");
    assert!(enriched.extensions().get::<KeycloakClaims>().is_some());

    let err = svc
        .call(Request::new(()))
        .expect_err("missing header must fail");
    assert_eq!(err.code(), Code::Unauthenticated);
}

#[test]
fn interceptor_is_clone_without_clonable_validator_or_claims() {
    // `KeyValidator` is not required to be `Clone`; only the `Arc` is cloned.
    const KID: &str = "clone-kid";
    let svc = JwtValidationService::<KeycloakClaims, _>::new(make_validator(KID));
    let mut cloned = svc.clone();

    cloned
        .call(bearer_request(KID))
        .expect("a cloned interceptor must validate tokens");
}

// -----------------------------------------------------------------------
// wrap_service: the wrapped service is still a NamedService
// -----------------------------------------------------------------------
#[test]
fn wrap_service_preserves_named_service() {
    let wrapped = wrap_service::<KeycloakClaims, _, _>(Named, make_validator("named-kid"));

    assert_eq!(name_of(&wrapped), "pkg.Named");
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
