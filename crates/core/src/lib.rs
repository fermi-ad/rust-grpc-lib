//! Authenticated gRPC for Fermilab controls services, built on
//! [tonic](https://docs.rs/tonic): clients attach a JWT to every request and
//! servers enforce per-method access policies before any handler runs.
//!
//! Use the re-exported [`tonic`], [`prost`] and [`async_trait`] instead of
//! direct dependencies so generated code and yours agree on versions. Services
//! run over plaintext (TLS is handled by infrastructure), and `test-util` ships
//! a publicly known key pair, so never enable it in production. See the
//! [guides](https://github.com/fermi-ad/rust-grpc-lib/tree/HEAD/docs).

// Maintainer notes.
//
// - Server-only dependencies are optional behind `server` so client-only
//   builds skip them; the client half is small and `server` needs most of it,
//   so it is always compiled.
// - Feature-gated modules also compile in unit tests, so their optional
//   dependencies are repeated under `[dev-dependencies]`.
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
#[cfg(any(feature = "server", test))]
pub mod telemetry;
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

    #[doc(hidden)]
    pub trait WireMethods {
        /// Canonical `package.Service/Method` paths, without the leading slash.
        const WIRE_METHODS: &'static [&'static str];
    }
}
