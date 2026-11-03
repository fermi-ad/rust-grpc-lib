//! Tests for the access policies of `#[keycloak_authenticated_service]`:
//! `#[roles(..)]` with typed roles, `#[any_authenticated]` and the `strict`
//! argument.
//!
//! # What is tested
//!
//! - A role written as a path to a `&str` constant is checked like a literal,
//!   for `any` and `all`, and can be mixed with literals.
//! - `#[any_authenticated]` admits any caller with claims, whatever their roles,
//!   and fails closed with `internal` when the claims are missing, for both
//!   parameter forms.
//! - A `strict` block in which every method declares a policy compiles and
//!   works, also when combined with `claims = ..`.

use tonic::{Code, Request, Response, Status};

use rust_grpc_lib::auth::{KeycloakClaims, RequestContext};
use rust_grpc_lib::keycloak_authenticated_service;
use rust_grpc_lib::testing::{claims_with_roles, request_with_claims};

/// Role names kept in one place, as a consumer would.
mod roles {
    pub const OPERATOR: &str = "operator";
    pub const ADMIN: &str = "admin";
}

fn claims(held: &[&str]) -> KeycloakClaims {
    claims_with_roles("caller", held)
}

fn with_claims(held: &[&str]) -> Request<String> {
    request_with_claims("hello".to_string(), claims(held))
}

fn without_claims() -> Request<String> {
    Request::new("hello".to_string())
}

trait PolicyService {
    async fn typed_any(&self, request: Request<String>) -> Result<Response<String>, Status>;
    async fn typed_all(&self, request: Request<String>) -> Result<Response<String>, Status>;
    async fn typed_ctx(&self, request: Request<String>) -> Result<Response<String>, Status>;
    async fn authenticated(&self, request: Request<String>) -> Result<Response<String>, Status>;
    async fn authenticated_ctx(&self, request: Request<String>)
    -> Result<Response<String>, Status>;
}

struct StrictServer;

// The `async fn` signatures are required: the macro only recognizes native
// async methods, so the handlers cannot be rewritten to return `ready(..)`.
#[allow(clippy::unused_async_trait_impl)]
#[keycloak_authenticated_service(strict, claims = rust_grpc_lib::auth::KeycloakClaims)]
impl PolicyService for StrictServer {
    #[roles(any(roles::OPERATOR, roles::ADMIN))]
    async fn typed_any(&self, request: Request<String>) -> Result<Response<String>, Status> {
        Ok(Response::new(request.into_inner()))
    }

    #[roles(all(roles::OPERATOR, "admin"))]
    async fn typed_all(&self, request: Request<String>) -> Result<Response<String>, Status> {
        Ok(Response::new(request.into_inner()))
    }

    #[roles(any(self::roles::ADMIN))]
    async fn typed_ctx(&self, ctx: RequestContext<String>) -> Result<Response<String>, Status> {
        Ok(Response::new(ctx.message))
    }

    #[any_authenticated]
    async fn authenticated(&self, request: Request<String>) -> Result<Response<String>, Status> {
        Ok(Response::new(request.into_inner()))
    }

    #[any_authenticated]
    async fn authenticated_ctx(
        &self,
        ctx: RequestContext<String>,
    ) -> Result<Response<String>, Status> {
        Ok(Response::new(ctx.message))
    }
}

#[tokio::test]
async fn typed_any_admits_a_holder_of_either_constant() {
    for held in [&["operator"][..], &["admin"][..]] {
        let response = StrictServer.typed_any(with_claims(held)).await.unwrap();

        assert_eq!(response.into_inner(), "hello");
    }
}

#[tokio::test]
async fn typed_any_denies_a_caller_without_either_constant() {
    let err = StrictServer
        .typed_any(with_claims(&["viewer"]))
        .await
        .unwrap_err();

    assert_eq!(err.code(), Code::PermissionDenied);
    assert_eq!(err.message(), "required role not present");
}

#[tokio::test]
async fn typed_all_mixes_a_constant_and_a_literal() {
    let allowed = StrictServer
        .typed_all(with_claims(&["operator", "admin"]))
        .await;
    let denied = StrictServer.typed_all(with_claims(&["operator"])).await;

    assert!(allowed.is_ok());
    assert_eq!(denied.unwrap_err().code(), Code::PermissionDenied);
}

#[tokio::test]
async fn typed_roles_work_on_a_request_context_parameter() {
    let allowed = StrictServer.typed_ctx(with_claims(&["admin"])).await;
    let denied = StrictServer.typed_ctx(with_claims(&["operator"])).await;

    assert!(allowed.is_ok());
    assert_eq!(denied.unwrap_err().code(), Code::PermissionDenied);
}

#[tokio::test]
async fn any_authenticated_admits_a_caller_with_no_roles() {
    let plain = StrictServer.authenticated(with_claims(&[])).await;
    let context = StrictServer.authenticated_ctx(with_claims(&[])).await;

    assert_eq!(plain.unwrap().into_inner(), "hello");
    assert_eq!(context.unwrap().into_inner(), "hello");
}

#[tokio::test]
async fn any_authenticated_fails_closed_without_claims() {
    let plain = StrictServer
        .authenticated(without_claims())
        .await
        .unwrap_err();
    let context = StrictServer
        .authenticated_ctx(without_claims())
        .await
        .unwrap_err();

    for err in [plain, context] {
        assert_eq!(err.code(), Code::Internal);
        assert_eq!(err.message(), "authentication is not configured");
    }
}
