//! Auth primitives for gRPC — re-exports from `rust-auth-lib` plus gRPC-specific wiring.
//!
//! Import everything you need from `rust_grpc_lib::auth`; a direct dependency on
//! `rust-auth-lib` is not required for gRPC use cases.
//!
//! # What is re-exported
//!
//! - **Traits** — [`TokenProvider`], [`TokenValidator`], [`Claims`], [`Ephemeral`]
//! - **Token sources** — [`FileTokenProvider`] (reads a platform-injected token
//!   file), [`ForwardedToken`] (wraps a raw token string as a provider)
//! - **Validators** — [`StaticKeysValidator`] / [`StaticKeysValidatorConfig`]
//!   (always available when the `auth` feature is enabled; backed by
//!   `rust-auth-lib`'s `jwks-local` implementation)
//! - **Error types** — [`AuthError`], [`ConfigError`], [`TokenError`]
//! - **Keycloak claims** — [`keycloak::KeycloakClaims`]
//! - **gRPC interceptors** — [`interceptor::ClientJwtInterceptor`] (used
//!   internally by the generated `from_endpoint_with_provider` constructors)
//! - **Server layer** — [`layer::JwtValidationLayer`] (type alias),
//!   [`layer::JwtValidationService`], [`validator_into_layer`] (constructor)
//! - **Token extraction** — [`extract_token`] (pulls a `Bearer` token out of an
//!   incoming tonic request and returns it as a [`ForwardedToken`])
//!
//! # What is intentionally omitted
//!
//! - **`KeycloakClientCredentialsProvider`** — not re-exported here because it
//!   requires a live Keycloak token endpoint and is not needed for the standard
//!   service archetypes. Use `rust-auth-lib` directly if you need it.
//! - **[`JwksValidator`] / [`JwksValidatorConfig`]** — only re-exported when the
//!   `jwks-url` feature is enabled. This feature is opt-in because it pulls in an
//!   HTTP client and background refresh task that most services do not need.

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

// Validators (always available when `auth` feature is enabled — uses `jwks-local`)
pub use rust_auth_lib::StaticKeysValidator;
pub use rust_auth_lib::StaticKeysValidatorConfig;

// Error types
pub use rust_auth_lib::AuthError;
pub use rust_auth_lib::ConfigError;
pub use rust_auth_lib::TokenError;

// Keycloak-specific claims
pub use rust_auth_lib::keycloak::KeycloakClaims;

// Live JWKS rotation — only when the `jwks-url` feature is enabled
#[cfg(feature = "jwks-url")]
pub use rust_auth_lib::JwksValidator;
#[cfg(feature = "jwks-url")]
pub use rust_auth_lib::JwksValidatorConfig;

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
