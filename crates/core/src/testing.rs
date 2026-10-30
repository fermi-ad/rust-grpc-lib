//! Test utilities for consumers of `rust-grpc-lib`.
//!
//! Enabled by the `test-util` feature, which is meant for `[dev-dependencies]`:
//!
//! ```toml
//! [dev-dependencies]
//! rust-grpc-lib = { git = "...", tag = "vX.Y.Z", features = ["test-util"] }
//! ```
//!
//! # What is here
//!
//! - [`KeycloakClaimsBuilder`] builds
//!   [`KeycloakClaims`](crate::auth::KeycloakClaims) with arbitrary subject,
//!   expiry and roles, and [`test_fixtures`] mints signed tokens against a
//!   test key pair. Both come from `rust-auth-lib`, so a direct dependency on
//!   it is not needed.
//! - [`RequestContext::for_test`](crate::auth::RequestContext::for_test) builds
//!   a context for a handler that takes `RequestContext<T>`.
//! - [`with_claims`] and [`request_with_claims`] put claims into a
//!   `tonic::Request`, for tests of handlers that still take `Request<T>` and
//!   read the claims from the extensions (or are guarded by
//!   `#[keycloak_authenticated_service]`).
//! - [`assert_all_rpcs_require_auth`] and [`assert_all_rpcs_require_role`] are
//!   black-box conformance checks that run against a live server.
//!
//! # Example
//!
//! ```rust,ignore
//! use rust_grpc_lib::auth::RequestContext;
//! use rust_grpc_lib::testing::{KeycloakClaimsBuilder, request_with_claims};
//!
//! let claims = KeycloakClaimsBuilder::new("alice").realm_role("operator").build();
//!
//! // A handler that takes `RequestContext<T>`:
//! let ctx = RequestContext::for_test(claims.clone(), SetDataRequest::default());
//! let reply = service.set_data(ctx).await?;
//!
//! // A handler that still takes `Request<T>`:
//! let request = request_with_claims(SetDataRequest::default(), claims);
//! let reply = service.set_data(request).await?;
//! ```
//!
//! Do not enable this feature in a production build.

use tonic::Request;

mod conformance;

pub use conformance::{assert_all_rpcs_require_auth, assert_all_rpcs_require_role, call_rpc};
pub use rust_auth_lib::test_fixtures;
pub use rust_auth_lib::test_fixtures::KeycloakClaimsBuilder;

#[cfg(test)]
mod tests;

/// Insert `claims` into the extensions of `request` and return it.
///
/// This is what the JWT validation layer does in production. Use it to test a
/// handler that takes `Request<T>` without running a server. `C` is normally
/// [`KeycloakClaims`](crate::auth::KeycloakClaims).
pub fn with_claims<T, C>(mut request: Request<T>, claims: C) -> Request<T>
where
    C: Clone + Send + Sync + 'static,
{
    request.extensions_mut().insert(claims);
    request
}

/// Build a `Request` around `message` that already carries `claims`.
///
/// Shorthand for `with_claims(Request::new(message), claims)`.
pub fn request_with_claims<T, C>(message: T, claims: C) -> Request<T>
where
    C: Clone + Send + Sync + 'static,
{
    with_claims(Request::new(message), claims)
}
