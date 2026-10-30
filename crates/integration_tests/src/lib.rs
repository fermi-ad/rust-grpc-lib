//! Integration test crate for `rust-grpc-lib`.
//!
//! This is the single Cargo integration-test entry point. All test modules are
//! declared here so that every dev-dependency is visible to the whole binary,
//! avoiding `unused_crate_dependencies` warnings.
//!
//! # Test modules
//!
//! - [`keycloak_authenticated_service_macro`] — unit-level tests for the `#[keycloak_authenticated_service]` macro
//!   expansion (role checks, missing claims, `any`/`all` variants).
//! - [`per_service_auth`] — per-service JWT auth via `wrap_service` and
//!   `with_interceptor`, with an unauthenticated health service alongside.
//! - [`integration_round_trip`] — full client→server gRPC round-trip tests
//!   using a real tonic server on a loopback port with `JwtValidationLayer`.
//! - [`trace_layer`] — `GrpcTraceLayer` spans, extensions and timeout handling
//!   on a live server, captured with a `tracing` layer.
//! - [`support`] — token, validator and span-capture helpers shared by the
//!   live-server tests.
//!
//! # Proto fixtures
//!
//! The `google` and `services` modules below include pre-generated Rust source
//! files from `src/fixtures/`. They are committed to the repository so that
//! `cargo test` requires no `build.rs` or live `protoc` invocation.
//!
//! The `services.example` generated code references
//! `super::super::google::protobuf::Timestamp`, so `google::protobuf` must
//! be declared at the crate root and `services::example` must be nested exactly
//! two levels deep from the root.
//!
//! To regenerate the fixtures after updating the `interface-definitions`
//! submodule, run `bash scripts/gen-test-fixtures.sh`.

#[cfg(test)]
mod compile_fail;
#[cfg(test)]
mod integration_round_trip;
#[cfg(test)]
mod keycloak_authenticated_service_macro;
#[cfg(test)]
mod macro_request_context;
#[cfg(test)]
mod per_service_auth;
#[cfg(test)]
mod support;
#[cfg(test)]
mod trace_layer;

#[cfg(test)]
pub mod google {
    pub mod protobuf {
        #![allow(clippy::all)]
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/fixtures/google.protobuf.rs"
        ));
    }
}

#[cfg(test)]
pub mod services {
    pub mod example {
        #![allow(clippy::all)]
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/fixtures/services.example.rs"
        ));
    }
}
