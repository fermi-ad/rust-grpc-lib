//! Test helpers for services built with this crate (`test-util` feature,
//! which implies `server`).
//!
//! **Enable it in `[dev-dependencies]` only.** It ships a publicly known RSA
//! key pair, and [`test_validator`] trusts anything signed with it; never let
//! it reach a production build. The
//! [service template](https://github.com/fermi-ad/rust-grpc-service-template)
//! is a runnable example service; see `docs/testing.md`.
//!
//! - **Handler unit tests** (no server; the access gate is bypassed, so they
//!   test handler logic, not access policy): [`claims_with_roles`] /
//!   [`KeycloakClaimsBuilder`] build claims;
//!   [`RequestContext::for_test`](crate::auth::RequestContext::for_test) and
//!   [`PublicContext::for_test`](crate::auth::PublicContext::for_test) build
//!   contexts (chain `with_rpc`, `with_remote_addr`, `with_metadata` or
//!   `with_extension` to simulate more call details; the contexts are
//!   read-only otherwise); [`authenticated_request`] / [`public_request`] build the
//!   `tonic::Request` that macro-rewritten handlers take.
//! - **Live-server tests:** [`test_validator`], [`test_token`],
//!   [`expired_test_token`], and the conformance checks
//!   [`assert_policy_table_conforms`] (start here),
//!   [`assert_service_conforms`], [`assert_all_rpcs_require_auth`],
//!   [`assert_all_rpcs_require_role`] and the primitive [`call_rpc`].
//! - **Health:** [`health_status`] and [`health_watch`] query the health
//!   service every `GrpcServer` runs, and [`ServingStatus`] is the status they
//!   report, so no `tonic-health` dependency is needed. Read the stream
//!   `health_watch` returns with [`StreamExt`], so no `futures-util`
//!   dependency is needed either.
//! - **Tracing:** [`capture_spans`] records span fields, such as those the
//!   trace layer and the gate record on the `rpc` span.
//!
//! [`KeycloakClaimsBuilder`] and [`test_fixtures`] are re-exported from
//! `rust-auth-lib`, so no direct dependency on it is needed.
//!
//! ```rust
//! use rust_grpc_lib::auth::RequestContext;
//! use rust_grpc_lib::testing::KeycloakClaimsBuilder;
//!
//! // Logic that takes the context itself can be called directly:
//! fn describe(ctx: &RequestContext<String>) -> String {
//!     format!("{} sent {:?}", ctx.sub(), ctx.message)
//! }
//!
//! let claims = KeycloakClaimsBuilder::new("alice").realm_role("operator").build();
//! let ctx = RequestContext::for_test(claims, "set_data".to_owned());
//! assert_eq!(describe(&ctx), "alice sent \"set_data\"");
//! assert!(ctx.has_role("operator"));
//! ```

use crate::auth::KeycloakClaims;

mod conformance;
mod health;
mod requests;
mod spans;
#[cfg(test)]
mod tests;
mod tokens;

pub use conformance::{
    assert_all_rpcs_require_auth, assert_all_rpcs_require_role, assert_policy_table_conforms,
    assert_service_conforms, call_rpc,
};
pub use futures_util::StreamExt;
pub use health::{ServingStatus, health_status, health_watch};
pub use requests::{authenticated_request, public_request};
pub use rust_auth_lib::test_fixtures;
pub use rust_auth_lib::test_fixtures::KeycloakClaimsBuilder;
pub use spans::{SpanCapture, SpanRecord, capture_spans};
pub use tokens::{expired_test_token, test_token, test_validator};

/// [`KeycloakClaims`] for `sub` holding the given realm roles, with the
/// builder's defaults otherwise. Use [`KeycloakClaimsBuilder`] for a custom
/// expiry or other claims.
#[must_use = "the claims are returned, not installed anywhere; pass them to `RequestContext::for_test`"]
pub fn claims_with_roles(sub: &str, roles: &[&str]) -> KeycloakClaims {
    roles
        .iter()
        .fold(KeycloakClaimsBuilder::new(sub), |builder, role| {
            builder.realm_role(*role)
        })
        .build()
}
