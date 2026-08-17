//! Integration test suite for rust-grpc-lib.
//!
//! This file is the single Cargo integration-test entry point.
//! All test modules are pulled in here so that every dev-dependency is
//! visible to the whole binary, avoiding `unused_crate_dependencies` warnings.

#[cfg(test)]
mod grpc_service_macro;
#[cfg(test)]
mod integration_round_trip;

// ---------------------------------------------------------------------------
// Include the pre-generated proto fixtures.
//
// The generated `services.devdb` module references
// `super::super::super::google::protobuf::Empty`, so we must nest it inside
// `services::devdb` and expose `google::protobuf` at the crate root level.
// ---------------------------------------------------------------------------
// Shared proto fixtures — declared here at the crate root so that the
// `super::super::super::google::protobuf::Empty` path in the generated
// DevDB code resolves correctly from within `integration_round_trip`.
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
    pub mod devdb {
        #![allow(clippy::all)]
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/fixtures/services.devdb.rs"
        ));
    }
}
