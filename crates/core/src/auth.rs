//! Re-exports from `rust-auth-lib` plus the two gRPC-specific sub-modules.
//!
//! Everything a consumer needs for JWT auth lives here; a direct dependency on
//! `rust-auth-lib` is not required.
//!
//! # Sub-modules
//!
//! - [`interceptor`] — [`ClientJwtInterceptor`], the outbound client interceptor
//!   that attaches `Bearer` tokens to every outgoing request.
//! - [`layer`] — [`JwtValidationLayer`] / [`JwtValidationService`] and the
//!   [`validator_into_layer`] constructor for server-side JWT validation.
//!
//! # Re-exported items
//!
//! **Traits:** [`TokenProvider`], [`TokenValidator`], [`Claims`], [`Ephemeral`]
//!
//! **Token sources:** [`FileTokenProvider`], [`ForwardedToken`]
//!
//! **Validators:** [`KeyValidator`], [`KeyValidatorConfig`]
//!
//! **Errors:** [`AuthError`], [`ConfigError`], [`TokenError`]
//!
//! **Claims:** [`KeycloakClaims`]
//!
//! **Free function:** [`extract_token`] — strips the `Bearer ` prefix from an
//! incoming tonic request's `Authorization` header and returns a
//! [`ForwardedToken`].
//!
//! # Intentionally omitted
//!
//! - `KeycloakClientCredentialsProvider` — requires a live Keycloak token
//!   endpoint; use `rust-auth-lib` directly if needed.

pub mod interceptor;
pub mod layer;

// Traits
pub use rust_auth_lib::Claims;
pub use rust_auth_lib::Ephemeral;
pub use rust_auth_lib::TokenProvider;
pub use rust_auth_lib::TokenValidator;

// Token sources
pub use rust_auth_lib::FileTokenProvider;
pub use rust_auth_lib::ForwardedToken;

// Validators
pub use rust_auth_lib::KeyValidator;
pub use rust_auth_lib::KeyValidatorConfig;

// Error types
pub use rust_auth_lib::AuthError;
pub use rust_auth_lib::ConfigError;
pub use rust_auth_lib::TokenError;

// Keycloak-specific claims
pub use rust_auth_lib::keycloak::KeycloakClaims;

// gRPC-specific types from submodules
pub use interceptor::ClientJwtInterceptor;
pub use layer::{JwtValidationLayer, validator_into_layer};
use tonic::Request;

#[cfg(test)]
mod tests;

/// Construct from the metadata of an incoming tonic request.
pub fn extract_token<T>(request: &Request<T>) -> Result<ForwardedToken, TokenError> {
    request
        .metadata()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(ForwardedToken::new)
        .ok_or(TokenError::EmptyAccessToken)
}
