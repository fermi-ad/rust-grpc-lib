//! Unpublished integration tests for `rust-grpc-lib` and its proc macros. A
//! separate crate because macro and generated code expand to `::rust_grpc_lib`
//! paths, so it stands in for a real consumer: no direct `tonic` or `prost`
//! dependency.
//!
//! `google` and `services` hold generated code from `src/fixtures/`; do not
//! edit by hand, regenerate with `bash scripts/gen-test-fixtures.sh`. The code
//! refers to `super::super::google::protobuf::Timestamp`, so keep
//! `google::protobuf` at the crate root and `services::example` two levels deep.

#[cfg(test)]
pub mod google {
    pub mod protobuf {
        #![allow(clippy::all, clippy::pedantic)]
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/fixtures/google.protobuf.rs"
        ));
    }
}

#[cfg(test)]
pub mod services {
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
mod telemetry;
#[cfg(test)]
mod trace_layer;
