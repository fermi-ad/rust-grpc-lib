//! Root of the `rust-grpc-lib` crate.
//!
//! Declares the public module tree and re-exports the proc-macro entry points
//! from `grpc-macro` so consumers never need a direct dependency on that crate.
//!
//! # Modules
//!
//! | Module | Feature gate | Purpose |
//! |---|---|---|
//! | [`auth`] | `auth` (default) | JWT interceptors, validation layer, token sources |
//! | `build_support` | `build` | Proto code-generation helpers for `build.rs` |
//! | [`pool`] | always | Process-wide lazily-connected channel pool |
//! | `server` | `server` | `GrpcServer::builder`: per-service auth, open health service, timeout, tracing and graceful shutdown |
//! | `testing` | `test-util` | Claims builders, `RequestContext::for_test`, and auth conformance helpers for tests |
//! | `trace` | `trace` | `GrpcTraceLayer`: a span per RPC, plus the `RpcMethod` and `RpcSpan` request extensions |
//!
//! # Re-exported macros
//!
//! | Item | Feature gate | What it generates |
//! |---|---|---|
//! | [`GrpcClient`] | `auth` (default) | `from_endpoint_with_provider` on tonic client structs |
//! | [`GrpcNoAuthClient`] | `unauthenticated` | `from_endpoint` (no-auth) on tonic client structs |
//! | [`keycloak_authenticated_service`] | `auth` (default) | Access guards on `impl Trait for Type` blocks, optionally `strict` |
//! | [`roles`], [`any_authenticated`] | `auth` (default) | The access policies a method declares; compile errors outside a `#[keycloak_authenticated_service]` block |
//!
//! See each module and the `grpc-macro` crate for full documentation.

#[cfg(any(feature = "auth", doc, test))]
pub mod auth;
#[cfg(any(feature = "build", test))]
pub mod build_support;
pub mod pool;
#[cfg(any(feature = "server", test))]
pub mod server;
#[cfg(any(feature = "test-util", test))]
pub mod testing;
#[cfg(any(feature = "trace", test))]
pub mod trace;

/// Derive macro that generates `from_endpoint_with_provider` on a tonic client
/// struct, wiring it into [`pool::get_channel`] with JWT auth.
///
/// Applied automatically by `build_support::Config::new` (`build` feature) to
/// every generated client. Re-exported here so consumers can also apply it to
/// custom wrapper types without a direct dependency on `grpc-macro`.
#[cfg(any(feature = "auth", doc, test))]
pub use grpc_macro::GrpcClient;

/// Derive macro that generates `from_endpoint` (no-auth) on a tonic client
/// struct. Intended for test harnesses only; requires the `unauthenticated`
/// feature.
#[cfg(any(feature = "unauthenticated", doc, test))]
pub use grpc_macro::GrpcNoAuthClient;

/// Attribute macro applied to `impl Trait for Type` blocks that injects
/// access guards into methods annotated with `#[roles(...)]` or
/// `#[any_authenticated]`. `#[keycloak_authenticated_service(strict)]`
/// makes a method without one of them a compile error.
///
/// Re-exported here so consumers can write `#[rust_grpc_lib::keycloak_authenticated_service]`
/// without a direct dependency on `grpc-macro`.
#[cfg(any(feature = "auth", doc, test))]
pub use grpc_macro::keycloak_authenticated_service;

/// Access policy: the caller must hold the listed roles. Each role is a string
/// literal or a path to a `&str` constant.
///
/// Consumed by [`keycloak_authenticated_service`]; a compile error anywhere
/// else. Re-exported so that mistake gets a clear message instead of
/// "cannot find attribute".
#[cfg(any(feature = "auth", doc, test))]
pub use grpc_macro::roles;

/// Access policy: any caller with valid claims is allowed, no role is needed.
///
/// Consumed by [`keycloak_authenticated_service`]; a compile error anywhere
/// else.
#[cfg(any(feature = "auth", doc, test))]
pub use grpc_macro::any_authenticated;
