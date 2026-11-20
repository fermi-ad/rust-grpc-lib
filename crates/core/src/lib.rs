//! Authenticated gRPC for Fermilab controls services, built on
//! [tonic](https://docs.rs/tonic): clients that attach a JWT to every request,
//! servers whose per-method access policies are enforced before any handler
//! runs, code generation for the bundled controls `.proto` files, and test
//! helpers. Import it as `rust_grpc_lib`.
//!
//! Servers get the tools to be secure: per-method access policies, enforced
//! by the gate that `GrpcServer::add_service` installs. Clients get the tools
//! to call the servers they need: [`GrpcClient`]'s `public` constructor sends
//! no token, and `authenticated` attaches one. The target method's policy
//! decides which to use, and the server's gate is the authority: a gated
//! method called without a valid token answers `Unauthenticated`.
//!
//! # Dependencies
//!
//! The consuming crate depends on `rust-grpc-lib` under that name (no
//! `package = ".."` rename), plus `tokio`, `tracing` and `tracing-subscriber`
//! as it needs them. It does **not** need `tonic`, `prost`, `tonic-prost`,
//! `async-trait` or `tonic-health` as direct dependencies: macro and
//! build-helper output refers only to `::rust_grpc_lib`, which re-exports what
//! it needs.
//!
//! - [`tonic`] and [`prost`] are re-exported whole, so write
//!   `rust_grpc_lib::tonic::{Request, Response, Status}` and
//!   `#[derive(rust_grpc_lib::prost::Message)]`.
//! - [`async_trait`] is tonic's re-export of the `async-trait` attribute, so
//!   service impls use `#[rust_grpc_lib::async_trait]`.
//! - With `test-util`, `testing` also provides health-check helpers and
//!   `StreamExt` to read their stream, so tests need no `tonic-health` or
//!   `futures-util`.
//!
//! These re-exports are the exact versions this crate was built against, so
//! generated code and your own code always agree on the tonic and prost types.
//! Use them rather than a direct `tonic` or `prost` dependency: one at a
//! different version gives types that do not match this crate's.
//!
//! # Features
//!
//! None is on by default. The client half is always compiled.
//!
//! | Feature | Enable it when | Adds |
//! |---|---|---|
//! | *(none)* | You call services | [`auth`] client types, [`pool`], [`GrpcClient`] (generates `public` and `authenticated` constructors), the [`tonic`], [`prost`] and [`async_trait`] re-exports |
//! | `server` | You serve services | `server`, the [`auth`] contexts and policy types, [`access_controlled_service`], [`roles`], [`any_authenticated`], [`public`] |
//! | `jwks-url` | Tokens are validated against a JWKS URL | `KeyValidatorConfig::from_jwks_url` |
//! | `build` | Your `build.rs` generates the bundled protos (`[build-dependencies]`) | `build_support` |
//! | `test-util` | Tests need tokens, contexts, health queries or live-server checks (`[dev-dependencies]` only); implies `server` | `testing`, `RequestContext::for_test`, `PublicContext::for_test` |
//!
//! `test-util` ships a publicly known RSA key pair: never enable it in a
//! production build.
//!
//! # Protos and TLS
//!
//! The bundled protos come only from the org-wide
//! [interface-definitions](https://github.com/fermi-ad/interface-definitions)
//! repository; you cannot add your own or select a subset, so every service
//! and client shares one set of generated types. Services operate over
//! plaintext: the infra team handles TLS, not this library, so do not enable
//! tonic's TLS features or configure TLS settings. Bearer tokens are never
//! logged.
//!
//! # Modules
//!
//! - [`auth`]: token providers, the client interceptor, validators, request
//!   contexts, access policies and how they are enforced.
//! - [`pool`]: the shared channel pool and its keep-alive settings.
//! - `server` (`server`): `GrpcServer::builder`, the gate, health and per-RPC
//!   tracing spans.
//! - `build_support` (`build`): `generate_protos` and `Config` for `build.rs`.
//! - `testing` (`test-util`): claims, contexts, tokens, conformance checks,
//!   health queries (with `testing::StreamExt` to read `health_watch`) and
//!   span capture.
//!
//! # Getting started
//!
//! The [service template](https://github.com/fermi-ad/rust-grpc-service-template)
//! is a runnable example service built on this crate. The
//! [guides](https://github.com/fermi-ad/rust-grpc-lib/tree/HEAD/docs) cover
//! concepts, clients, servers, testing and troubleshooting.

// Maintainer notes.
//
// - Server-only dependencies are optional behind `server` so client-only
//   builds skip them; the client half is small and `server` needs most of it,
//   so it is always compiled.
// - `server`, `trace` (private), `testing` and `build_support` are gated with
//   `any(feature = "..", test)` so unit tests build without features;
//   `Cargo.toml` repeats their optional dependencies under
//   `[dev-dependencies]`. `http-body` there is only for `trace`'s unit tests.
// - Crate docs link only to items present in every feature combination.
//   Feature-gated items are plain code spans: a link would break without the
//   feature. The macro re-exports add a `doc` arm to their gate to stay
//   linkable; real builds without `server` do not expose them.
// - The macro re-exports carry no docs here; rustdoc inlines grpc_macro's.
// - `tonic` and `prost` are re-exported so consumers need neither as a direct
//   dependency. `tonic-prost` is a non-optional dependency (it is only a few
//   small modules on top of `tonic` and `prost`) because generated code names
//   its `ProstCodec`, and it is reachable only through `__private`, which is
//   not public API: the macros and `build_support` output depend on it, so
//   keep it in sync with them.
// - Generated code and the macros name `::rust_grpc_lib::__private::..`. The
//   `build_support` post-processing step rewrites tonic-build's `tonic::`
//   paths to that form, because tonic-build cannot be configured to emit
//   another prefix.

pub mod auth;
#[cfg(any(feature = "build", test))]
pub mod build_support;
pub mod pool;
#[cfg(any(feature = "server", test))]
pub mod server;
#[cfg(any(feature = "test-util", test))]
pub mod testing;
#[cfg(any(feature = "server", test))]
mod trace;

pub use grpc_macro::GrpcClient;
#[cfg(any(feature = "server", doc, test))]
pub use grpc_macro::access_controlled_service;
#[cfg(any(feature = "server", doc, test))]
pub use grpc_macro::any_authenticated;
#[cfg(any(feature = "server", doc, test))]
pub use grpc_macro::public;
#[cfg(any(feature = "server", doc, test))]
pub use grpc_macro::roles;

pub use prost;
pub use tonic;
pub use tonic::async_trait;

/// Implementation detail of the macros and generated code. Not public API: it
/// can change in any release.
#[doc(hidden)]
pub mod __private {
    pub use ::prost;
    pub use ::tonic;
    pub use ::tonic_prost;
}
