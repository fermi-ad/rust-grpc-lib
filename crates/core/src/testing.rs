//! Test helpers for services built with this crate (`test-util` feature).
//! Enable it in `[dev-dependencies]` only: it ships a publicly known RSA key
//! pair that [`test_validator`] trusts.
//!
//! Handler unit tests bypass the access gate, so they test handler logic, not
//! policy; use [`TestServer`] and the conformance checks for policy.
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
mod metrics;
mod requests;
mod server;
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
pub use metrics::{CounterSnapshot, HistogramSnapshot, InMemoryMetrics};
pub use requests::{authenticated_request, public_request};
pub use rust_auth_lib::test_fixtures;
pub use rust_auth_lib::test_fixtures::KeycloakClaimsBuilder;
pub use server::{TestServer, TestServerBuilder};
pub use spans::{SpanCapture, SpanRecord, capture_spans};
pub use tokens::{expired_test_token, test_token, test_validator};

/// Uses [`KeycloakClaimsBuilder`]'s defaults otherwise.
#[must_use = "the claims are returned, not installed anywhere; pass them to `RequestContext::for_test`"]
pub fn claims_with_roles(sub: &str, roles: &[&str]) -> KeycloakClaims {
    roles
        .iter()
        .fold(KeycloakClaimsBuilder::new(sub), |builder, role| {
            builder.realm_role(*role)
        })
        .build()
}
