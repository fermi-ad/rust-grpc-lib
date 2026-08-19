//! gRPC connection pool, zero-trust JWT authentication, and code-generation
//! helpers for Controls group services.
//!
//! This library serves three service archetypes:
//!
//! - **Middle-layer services** — validate incoming user JWTs with
//!   [`auth::JwtValidationLayer`], enforce role-based access control via
//!   `#[grpc_service]` / `#[roles(...)]`, and forward the upstream token to
//!   downstream gRPC services using [`auth::ForwardedTokenInterceptor`].
//! - **Edge/hardware services** — authenticate outbound calls with a
//!   platform-injected token file via [`auth::FileTokenProvider`] and
//!   [`pool::get`].
//! - **Test harnesses** — bypass auth entirely with [`pool::get_unauthenticated`]
//!   (requires the `unauthenticated` feature).
//!
//! # Feature flags
//!
//! | Feature | Default | Description |
//! |---|---|---|
//! | `auth` | ✅ on | Enables JWT auth; [`pool::get`] requires a [`auth::TokenProvider`] |
//! | `jwks-url` | off | Enables live JWKS rotation via [`auth::JwksValidator`] (opt-in) |
//! | `unauthenticated` | off | Enables [`pool::get_unauthenticated`] with no auth; for test harnesses only |
//! | `build` | off | Enables proto code-generation helpers ([`build_support`] module) |
//!
//! # Code generation is the consumer's responsibility
//!
//! This library does **not** expose a `proto` module of its own. Instead, you
//! call [`build_support::generate_protos`] from your own `build.rs`, then
//! include the output in your crate:
//!
//! ```rust,ignore
//! // build.rs
//! use rust_grpc_lib::build_support::{ Config, generate_protos };
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let config = Config::new();
//!     // < optional calls to configure custom attributes here >
//!     generate_protos(config)?;
//!     Ok(())
//! }
//! ```
//!
//! ```rust,ignore
//! // src/proto.rs  (or inline in src/lib.rs)
//! include!(concat!(env!("OUT_DIR"), "/proto.rs"));
//! ```
//!
//! # Runtime requirement
//!
//! This library requires a [Tokio](https://tokio.rs) runtime. Tonic's transport
//! layer is built on Tokio; there is no way to use it without one.
//!
//! # Quick start — middle-layer service
//!
//! After setting up code generation (see above), install [`auth::JwtValidationLayer`]
//! on your server and use [`pool::get`] with [`auth::ForwardedTokenInterceptor`]
//! to call downstream services:
//!
//! ```rust,ignore
//! use std::sync::Arc;
//! use rust_grpc_lib::auth::{
//!     StaticKeysValidator, StaticKeysValidatorConfig,
//!     JwtValidationLayer, ForwardedTokenInterceptor,
//! };
//!
//! // Reads AUTH_JWKS_FILE or AUTH_PEM_FILE; optionally AUTH_ISSUER
//! let validator = Arc::new(StaticKeysValidator::new(StaticKeysValidatorConfig::from_env()?)?);
//!
//! tonic::transport::Server::builder()
//!     .layer(JwtValidationLayer::new(validator))
//!     .add_service(MyServiceServer::new(MyService))
//!     .serve("[::1]:50051".parse()?)
//!     .await?;
//! ```
//!
//! # Quick start — edge/hardware service
//!
//! Use [`auth::FileTokenProvider`] to authenticate outbound calls with a
//! platform-injected token file:
//!
//! ```rust,ignore
//! use rust_grpc_lib::auth::FileTokenProvider;
//!
//! // Reads SERVICE_TOKEN_FILE; caches for SERVICE_TOKEN_CACHE_TTL_SECS (default 30 s)
//! let provider = FileTokenProvider::from_env()?;
//! let client: MyServiceClient<_> = rust_grpc_lib::pool::get("http://host:50051", provider)?;
//! ```
//!
//! # Connection pooling
//!
//! Channels are keyed by the endpoint string. Calling [`pool::get`] multiple
//! times with the same endpoint returns clients that share the same underlying
//! channel. Connections are established lazily on the first RPC, not at the
//! time [`pool::get`] is called.
//!
//! # Adding derive macros to generated types
//!
//! If you need generated structs or enums to implement additional traits (e.g.
//! `serde::Serialize`), make the relevant calls on your [`Config`](crate::build_support::Config) before building:
//!
//! ```rust,ignore
//! // build.rs
//! use rust_grpc_lib::build_support::{ Config, generate_protos };
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let mut config = Config::new();
//!     config = config.type_attribute("some.package", "#[derive(my_macro)]");
//!     config = config.client_attribute("other.package", "#[derive(client_only_macro)]");
//!     generate_protos(config)?;
//!     Ok(())
//! }
//! ```
//!
//! # Keepalive configuration
//!
//! HTTP/2 keepalive probes are sent on every channel. The timing can be
//! overridden at runtime via environment variables:
//!
//! | Variable | Default | Meaning |
//! |---|---|---|
//! | `RUST_GRPC_LIB_KEEP_ALIVE_INTERVAL_SECS` | `30` | Seconds between keepalive pings |
//! | `RUST_GRPC_LIB_KEEP_ALIVE_TIMEOUT_SECS` | `10` | Seconds to wait for a keepalive ping acknowledgement before closing the connection |
//!
//! Keepalive pings are also sent while the connection is idle.

#[cfg(feature = "auth")]
use tonic::service::Interceptor;
use tonic::transport::Channel;

#[cfg(any(feature = "build", doc, test))]
pub mod build_support;

pub mod pool;

/// Auth primitives — JWT interceptors, validation layers, and re-exports from
/// `rust-auth-lib`. Available when the `auth` feature is enabled (the default).
#[cfg(feature = "auth")]
pub mod auth;

/// Re-export of the [`GrpcClient`] derive macro so consumers can use
/// `#[derive(rust_grpc_lib::GrpcClient)]` without adding a separate
/// dependency on the proc-macro crate.
pub use grpc_macro::GrpcClient;

/// Re-export of the `grpc_service` proc macro for annotating gRPC service
/// implementations with JWT auth wiring.
#[cfg(feature = "auth")]
pub use grpc_macro::grpc_service;

/// Marker trait for gRPC client types that can be constructed from a [`Channel`].
///
/// All generated tonic service clients implement this trait automatically via
/// the bundled `#[derive(GrpcClient)]` macro — you do not need to implement it
/// yourself unless you are wrapping a generated client in your own newtype.
#[diagnostic::on_unimplemented(
    message = "`{Self}` does not implement `GrpcClient`",
    label = "`{Self}` is not a generated service client from this library",
    note = "Client types generated by rust-grpc-lib implement GrpcClient by default. \
        Manually implement it for your client if it is not from rust-grpc-lib."
)]
pub trait GrpcClient: Sized {
    /// Construct a client from an existing [`Channel`].
    fn from_channel(channel: Channel) -> Self;

    /// Construct a client from an existing [`Channel`] wrapped with a
    /// [`Interceptor`].
    ///
    /// Used by [`pool::get`] when the `auth` feature is active to attach a
    /// [`crate::auth::interceptor::ClientJwtInterceptor`] to every outbound
    /// request.
    #[cfg(feature = "auth")]
    fn from_channel_with_interceptor<I>(channel: Channel, interceptor: I) -> Self
    where
        I: Interceptor + Send + Sync + 'static;
}
