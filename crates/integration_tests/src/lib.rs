//! Integration tests for `rust-grpc-lib` and its proc macros. Unpublished;
//! exports nothing.
//!
//! A separate crate because the macros and generated code expand to
//! `::rust_grpc_lib` paths, so they only work from a consumer crate. It
//! stands in for a real consumer: like one, it has no direct `tonic`,
//! `prost`, `tonic-prost`, `async-trait` or `tonic-health` dependency, and
//! uses `rust_grpc_lib::tonic`, `#[rust_grpc_lib::async_trait]` and
//! `rust_grpc_lib::testing`'s health helpers and `StreamExt` instead (no
//! `futures-util` either). All modules are
//! `#[cfg(test)]` and build into one test binary (`cargo test -p
//! integration_tests`); they need the `test-util` and `server` features
//! enabled in `Cargo.toml`.
//!
//! Modules (`#[cfg(test)]`, so not linkable): `compile_fail` (trybuild
//! diagnostics), `access_controlled_service_macro` (expansion, no server),
//! `method_name_matching` (macro-emitted names against the gate's lookup),
//! `access_gate`, `server_builder`, `trace_layer` (live servers), and
//! `support` (shared helpers).
//!
//! # Proto fixtures
//!
//! `google` and `services` include generated code from `src/fixtures/`,
//! committed so no `build.rs` or `protoc` is needed. Do not edit by hand;
//! regenerate with `bash scripts/gen-test-fixtures.sh`. The generated code
//! refers to `super::super::google::protobuf::Timestamp`, so keep
//! `google::protobuf` at the crate root and `services::example` two levels
//! deep.

/// Generated code for the `google` proto package.
#[cfg(test)]
pub mod google {
    /// Generated code for `google.protobuf`.
    pub mod protobuf {
        #![allow(clippy::all, clippy::pedantic)]
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/fixtures/google.protobuf.rs"
        ));
    }
}

/// Generated code for the `services` proto packages.
#[cfg(test)]
pub mod services {
    /// Generated code for `services.example`, the `ExampleDB` service.
    pub mod example {
        #![allow(clippy::all, clippy::pedantic)]
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/fixtures/services.example.rs"
        ));
    }
}

#[cfg(test)]
mod access_controlled_service_macro;
#[cfg(test)]
mod access_gate;
#[cfg(test)]
mod compile_fail;
#[cfg(test)]
mod method_name_matching;
#[cfg(test)]
mod server_builder;
#[cfg(test)]
mod support;
#[cfg(test)]
mod trace_layer;
