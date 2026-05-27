//! Build-time helpers for compiling the bundled `.proto` definitions into Rust.
//!
//! The public entry point is [`generate_protos`], which consumers call from
//! their own `build.rs`. Everything else in this module is an implementation
//! detail and is not part of the public API.

mod codegen;
mod file_utils;
mod proto_rs;

use std::{env, error::Error, ffi::OsStr, fs, path::Path};

/// Compile the bundled `.proto` definitions and write a `proto.rs` into your
/// crate's `OUT_DIR`.
///
/// Call this from your own `build.rs` to generate Rust types for all Controls
/// gRPC services. The `.proto` files are shipped with this library, so you do
/// not need to vendor them yourself.
///
/// # Setup
///
/// Add `rust-grpc-lib` with the `build` feature to your build dependencies:
///
/// ```toml
/// [build-dependencies]
/// rust-grpc-lib = { git = "https://github.com/fermi-ad/rust-grpc-lib", tag = "vX.Y.Z", features = ["build"] }
/// ```
///
/// Then create a `build.rs` at the root of your crate:
///
/// ```rust,ignore
/// fn main() -> Result<(), Box<dyn std::error::Error>> {
///     rust_grpc_lib::build_support::generate_protos()?;
///     Ok(())
/// }
/// ```
///
/// # Using the generated types
///
/// The function writes a single file, `proto.rs`, into the directory given by
/// the `OUT_DIR` environment variable (set automatically by Cargo). Include it
/// wherever you want the generated module to live:
///
/// ```rust,ignore
/// // src/proto.rs  — or inline in src/lib.rs
/// include!(concat!(env!("OUT_DIR"), "/proto.rs"));
/// ```
///
/// After that, all generated message types and service clients are accessible
/// through that module:
///
/// ```rust,ignore
/// mod proto {
///     include!(concat!(env!("OUT_DIR"), "/proto.rs"));
/// }
///
/// use proto::services::alarm_commands::alarm_commands_client::AlarmCommandsClient;
/// ```
///
/// # Generated module layout
///
/// | Module | Contents |
/// |---|---|
/// | `common` | Shared message types (alarms, devices, DRF, events, sources, status) |
/// | `services` | Per-service request/response types and tonic client structs |
/// | `google::protobuf` | Well-known types (`Timestamp`, `Duration`, `Any`, …) |
/// | `third_party` | Third-party proto types vendored alongside the Controls protos |
/// | `dpm` | **Deprecated.** Use `services::daq` instead. |
///
/// # Customising generated attributes
///
/// Three environment variables let you attach extra Rust attributes to
/// generated types without forking this library. Each variable holds a
/// semicolon-separated list of `proto.path=attribute` pairs:
///
/// | Variable | `tonic-prost-build` method |
/// |---|---|
/// | `RUST_GRPC_LIB_ENUM_ATTRIBUTES` | `enum_attribute` |
/// | `RUST_GRPC_LIB_FIELD_ATTRIBUTES` | `field_attribute` |
/// | `RUST_GRPC_LIB_TYPE_ATTRIBUTES` | `type_attribute` |
///
/// Example — add `serde` derives to every type in the `common.alarm` package:
///
/// ```text
/// RUST_GRPC_LIB_TYPE_ATTRIBUTES="common.alarm=#[derive(serde::Serialize, serde::Deserialize)]"
/// ```
///
/// # Safety note
///
/// This function calls [`std::env::set_var`] to point `PROTOC` at the vendored
/// `protoc` binary. That call is only safe when no other threads are reading
/// the environment concurrently. Cargo runs `build.rs` in a single-threaded
/// process, so this is safe in normal usage. Do **not** call this function from
/// a multi-threaded context.
///
/// # Errors
///
/// Returns an error if:
/// - the bundled `.proto` files cannot be read,
/// - `tonic-prost-build` fails to compile the protos,
/// - `OUT_DIR` is not set (i.e. the function is called outside of a build script), or
/// - an environment variable contains a malformed attribute pair.
pub fn generate_protos() -> Result<(), Box<dyn Error>> {
    // `env!("CARGO_MANIFEST_DIR")` is resolved at compile time of *this* crate
    // and baked into the binary. Because the `.proto` files are included in the
    // published crate via the `include` field in Cargo.toml, this path is valid
    // on the consumer's machine even though it points into this crate's source.
    let interface_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/interface-definitions");
    let mut proto_dir = interface_dir.to_string();
    proto_dir.push_str("/proto");
    let proto_files = file_utils::find_proto_files(&proto_dir, &OsStr::new("proto"))?;

    codegen::compile(interface_dir, &proto_files)?;

    let packages = file_utils::collect_packages(&proto_files)?;
    let proto_rs_source = proto_rs::build_contents(&packages);

    let out_dir = env::var("OUT_DIR")?;
    let out_path = Path::new(&out_dir).join("proto.rs");
    fs::write(&out_path, proto_rs_source)?;

    Ok(())
}
