//! Integration tests for the `#[grpc_service]` proc-macro expansion.
//!
//! These tests live in `crates/core/tests/` (an integration-test crate) so
//! that the generated code's `::rust_grpc_lib::auth::KeycloakClaims` path
//! resolves correctly — the macro is designed to be used from *consumer*
//! crates, not from inside `rust-grpc-lib` itself.
//!
//! # What is tested
//!
//! 1. **Compile-time** — the macro expands without error on a minimal
//!    tonic-like service impl (verified by the fact that this file compiles).
//! 2. **Valid role** — calling the generated method with a request whose
//!    extensions contain `KeycloakClaims` that carry the required role
//!    succeeds (returns `Ok`).
//! 3. **Missing role** — calling with claims that do *not* carry the required
//!    role returns `Err(Status::permission_denied(...))`.
//! 4. **No claims** — calling with a request that has no `KeycloakClaims` in
//!    its extensions returns `Err(Status::internal(...))`.
//! 5. **`all(...)` variant** — every listed role must be present; partial
//!    matches are rejected.

use std::time::{SystemTime, UNIX_EPOCH};

use tonic::{Code, Request, Response, Status};

use rust_grpc_lib::auth::KeycloakClaims;

// ---------------------------------------------------------------------------
// Minimal tonic-like service trait and server struct
// ---------------------------------------------------------------------------

/// A minimal stand-in for a tonic-generated service trait.
trait EchoService {
    async fn echo(&self, request: Request<String>) -> Result<Response<String>, Status>;
}

/// Concrete server type that the `any(...)` variant of the macro is applied to.
struct EchoServer;

// Apply the macro.  The `#[roles(any("admin"))]` annotation on `echo` causes
// `#[grpc_service]` to inject a guard that:
//   1. Retrieves `KeycloakClaims` from `request.extensions()`.
//   2. Returns `Status::internal` if the claims are absent.
//   3. Returns `Status::permission_denied` if the `"admin"` role is absent.
//   4. Falls through to the original body otherwise.
#[rust_grpc_lib::grpc_service]
impl EchoService for EchoServer {
    #[roles(any("admin"))]
    async fn echo(&self, request: Request<String>) -> Result<Response<String>, Status> {
        // Original body — only reached when the role check passes.
        let msg = request.into_inner();
        Ok(Response::new(format!("echo: {msg}")))
    }
}

/// Concrete server type that the `all(...)` variant of the macro is applied to.
struct MultiRoleServer;

#[rust_grpc_lib::grpc_service]
impl EchoService for MultiRoleServer {
    #[roles(all("admin", "operator"))]
    async fn echo(&self, request: Request<String>) -> Result<Response<String>, Status> {
        Ok(Response::new(request.into_inner()))
    }
}

// ---------------------------------------------------------------------------
// Helper: build a KeycloakClaims value with the given realm roles.
//
// KeycloakClaims has private fields and no public constructor, so we
// deserialize it from a JSON payload that matches its serde shape.
// ---------------------------------------------------------------------------
fn make_claims(realm_roles: &[&str]) -> KeycloakClaims {
    let roles_json: Vec<serde_json::Value> =
        realm_roles.iter().map(|r| serde_json::json!(r)).collect();

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let json = serde_json::json!({
        "sub": "test-user",
        "iat": now,
        "exp": now + 3600,
        "realm_access": { "roles": roles_json }
    });

    serde_json::from_value(json).expect("KeycloakClaims deserialization must succeed")
}

// ---------------------------------------------------------------------------
// Test 1: valid role → method body executes and returns Ok
// ---------------------------------------------------------------------------
#[tokio::test]
async fn valid_role_allows_method_to_execute() {
    let svc = EchoServer;

    let mut req = Request::new("hello".to_string());
    req.extensions_mut().insert(make_claims(&["admin"]));

    let result = svc.echo(req).await;

    let resp = result.expect("method must succeed when the required role is present");
    assert_eq!(resp.into_inner(), "echo: hello");
}

// ---------------------------------------------------------------------------
// Test 2: missing role → permission_denied
// ---------------------------------------------------------------------------
#[tokio::test]
async fn missing_role_returns_permission_denied() {
    let svc = EchoServer;

    let mut req = Request::new("hello".to_string());
    // Claims present but "admin" role is absent.
    req.extensions_mut().insert(make_claims(&["operator"]));

    let err = svc
        .echo(req)
        .await
        .expect_err("method must fail when the required role is absent");

    assert_eq!(
        err.code(),
        Code::PermissionDenied,
        "status must be PERMISSION_DENIED when the caller lacks the required role"
    );
}

// ---------------------------------------------------------------------------
// Test 3: no claims in extensions → internal error
// ---------------------------------------------------------------------------
#[tokio::test]
async fn no_claims_in_extensions_returns_internal() {
    let svc = EchoServer;

    // No KeycloakClaims inserted — simulates a missing JwtValidationLayer.
    let req = Request::new("hello".to_string());

    let err = svc
        .echo(req)
        .await
        .expect_err("method must fail when KeycloakClaims are absent from extensions");

    assert_eq!(
        err.code(),
        Code::Internal,
        "status must be INTERNAL when JwtValidationLayer has not populated the claims"
    );
}

// ---------------------------------------------------------------------------
// Test 4: all(...) — every listed role must be present
// ---------------------------------------------------------------------------
#[tokio::test]
async fn all_roles_present_allows_execution() {
    let svc = MultiRoleServer;
    let mut req = Request::new("hi".to_string());
    req.extensions_mut()
        .insert(make_claims(&["admin", "operator"]));

    let result = svc.echo(req).await;
    assert!(
        result.is_ok(),
        "must succeed when all required roles are present"
    );
}

#[tokio::test]
async fn all_roles_partial_match_returns_permission_denied() {
    let svc = MultiRoleServer;
    let mut req = Request::new("hi".to_string());
    // Only "admin" — "operator" is missing.
    req.extensions_mut().insert(make_claims(&["admin"]));

    let err = svc
        .echo(req)
        .await
        .expect_err("must fail when only some of the required roles are present");

    assert_eq!(
        err.code(),
        Code::PermissionDenied,
        "status must be PERMISSION_DENIED when not all required roles are present"
    );
}
