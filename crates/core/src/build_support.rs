//! Compiles the bundled controls `.proto` files into tonic and prost code from
//! a consumer's `build.rs` (`build` feature).

use std::{env, error::Error, ffi::OsStr, fs, path::Path};

use protoc_bin_vendored::protoc_bin_path;
use tonic_prost_build::Builder;

mod file_utils;
mod proto_rs;
mod rewrite;

// Generated code names `prost` and `tonic_prost` through `rust-grpc-lib`, so
// the consuming crate needs neither as a dependency. The `tonic` paths cannot
// be configured; `rewrite` fixes them after the fact.
const PROST_PATH: &str = "::rust_grpc_lib::__private::prost";
const CODEC_PATH: &str = "::rust_grpc_lib::__private::tonic_prost::ProstCodec";

#[cfg(test)]
mod tests;

/// Compiles every bundled `.proto` file and writes `$OUT_DIR/proto.rs`.
///
/// The protos always compile as a whole set: they come only from the org-wide
/// interface-definitions repository so every service shares one set of types.
/// A proto `package a.b;` becomes `proto::a::b`. Overwrites `PROTOC`, so call
/// it only from a single-threaded process such as a build script.
///
/// ```no_run
/// // build.rs
/// use rust_grpc_lib::build_support::{Config, generate_protos};
///
/// fn main() -> Result<(), Box<dyn std::error::Error>> {
///     generate_protos(Config::new())
/// }
/// ```
///
/// # Errors
///
/// Fails if a proto cannot be read or has no `package` line, if `protoc` or
/// post-processing fails, or if `OUT_DIR` is unset or `proto.rs` cannot be written.
pub fn generate_protos(config: Config) -> Result<(), Box<dyn Error>> {
    // `CARGO_MANIFEST_DIR` is fixed when this crate is compiled. It stays valid
    // in a consumer's build because the `include` list in Cargo.toml ships the
    // protos with the published crate, so the path points into the unpacked
    // dependency.
    let interface_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/interface-definitions");
    let mut proto_dir = interface_dir.to_string();
    proto_dir.push_str("/proto");
    let proto_files = file_utils::find_proto_files(&proto_dir, OsStr::new("proto"))?;

    // `collect_packages` sorts, so the contents of `proto.rs` do not depend on
    // the order in which the directory walk returned the files.
    let packages = file_utils::collect_packages(&proto_files)?;

    let out_dir = env::var("OUT_DIR")?;
    let out_dir = Path::new(&out_dir);

    compile(interface_dir, &proto_files, &packages, config, out_dir)?;

    let proto_rs_source = proto_rs::build_contents(&packages);
    fs::write(out_dir.join("proto.rs"), proto_rs_source)?;

    Ok(())
}

/// Used by `scripts/gen-test-fixtures.sh`; not public API. Like
/// [`generate_protos`] but writes only the `<package>.rs` files, no `proto.rs`.
#[doc(hidden)]
pub fn generate_fixtures(
    proto_files: &[String],
    include_dir: &str,
    out_dir: &Path,
    config: Config,
) -> Result<(), Box<dyn Error>> {
    fs::create_dir_all(out_dir)?;
    let packages = file_utils::collect_packages(proto_files)?;
    compile(include_dir, proto_files, &packages, config, out_dir)
}

/// Extra attributes for the generated code; pass it to [`generate_protos`].
///
/// ```no_run
/// // build.rs
/// use rust_grpc_lib::build_support::{Config, generate_protos};
///
/// fn main() -> Result<(), Box<dyn std::error::Error>> {
///     generate_protos(
///         Config::new()
///             .type_attribute(".common.alarm", "#[derive(serde::Serialize)]")
///             .server_mod_attribute("services.example", "#[allow(clippy::too_many_arguments)]"),
///     )
/// }
/// ```
///
/// Paths are **protobuf** names, not Rust paths; `"."` matches everything. The
/// type and field methods take a package, message, enum or field, and a leading
/// `.` anchors at the root. The client/server methods take `package.Service`
/// and the `*_mod_attribute` methods a package, never with a leading `.`.
pub struct Config {
    builder: Builder,
}

impl Config {
    /// Defaults that cannot be removed: rerun on proto change, well-known types
    /// are generated, and every client derives [`GrpcClient`](crate::GrpcClient).
    #[must_use = "a `Config` does nothing until it is passed to `generate_protos`"]
    pub fn new() -> Self {
        let builder = tonic_prost_build::configure()
            .codec_path(CODEC_PATH)
            .emit_rerun_if_changed(true)
            .compile_well_known_types(true)
            .client_attribute(".", "#[derive(::rust_grpc_lib::GrpcClient)]");

        Config { builder }
    }

    /// Targets the client struct (such as `ExampleDbClient<T>`) of each matching service.
    #[must_use = "this consumes the `Config` and returns an updated one; the attribute is lost if the result is dropped"]
    pub fn client_attribute(self, path: &str, attribute: &str) -> Self {
        Config {
            builder: self.builder.client_attribute(path, attribute),
        }
    }

    /// Targets the client modules (`*_client`) of each matching package.
    #[must_use = "this consumes the `Config` and returns an updated one; the attribute is lost if the result is dropped"]
    pub fn client_mod_attribute(self, path: &str, attribute: &str) -> Self {
        Config {
            builder: self.builder.client_mod_attribute(path, attribute),
        }
    }

    /// Targets matching enums, including `oneof` enums (`package.Message.oneof_name`).
    #[must_use = "this consumes the `Config` and returns an updated one; the attribute is lost if the result is dropped"]
    pub fn enum_attribute(self, path: &str, attribute: &str) -> Self {
        Config {
            builder: self.builder.enum_attribute(path, attribute),
        }
    }

    /// Targets matching message fields, enum variants and `oneof` variants.
    #[must_use = "this consumes the `Config` and returns an updated one; the attribute is lost if the result is dropped"]
    pub fn field_attribute(self, path: &str, attribute: &str) -> Self {
        Config {
            builder: self.builder.field_attribute(path, attribute),
        }
    }

    /// Targets the structs of matching messages, not enums.
    #[must_use = "this consumes the `Config` and returns an updated one; the attribute is lost if the result is dropped"]
    pub fn message_attribute(self, path: &str, attribute: &str) -> Self {
        Config {
            builder: self.builder.message_attribute(path, attribute),
        }
    }

    /// Targets the server struct (such as `ExampleDbServer<T>`) of each matching
    /// service, not its trait.
    #[must_use = "this consumes the `Config` and returns an updated one; the attribute is lost if the result is dropped"]
    pub fn server_attribute(self, path: &str, attribute: &str) -> Self {
        Config {
            builder: self.builder.server_attribute(path, attribute),
        }
    }

    /// Targets the server modules (`*_server`) of each matching package.
    #[must_use = "this consumes the `Config` and returns an updated one; the attribute is lost if the result is dropped"]
    pub fn server_mod_attribute(self, path: &str, attribute: &str) -> Self {
        Config {
            builder: self.builder.server_mod_attribute(path, attribute),
        }
    }

    /// Targets matching messages and enums, including `oneof` enums.
    #[must_use = "this consumes the `Config` and returns an updated one; the attribute is lost if the result is dropped"]
    pub fn type_attribute(self, path: &str, attribute: &str) -> Self {
        Config {
            builder: self.builder.type_attribute(path, attribute),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self::new()
    }
}

// Runs the vendored protoc over `proto_files`, writing `<package>.rs` files
// into `out_dir`, then rewrites their `tonic::` paths (see `rewrite`).
// `packages` are the proto package names of `proto_files`. `parent_dir` is the
// include root because the protos import each other as `proto/controls/...`,
// relative to `interface-definitions`.
fn compile(
    parent_dir: &str,
    proto_files: &[String],
    packages: &[String],
    config: Config,
    out_dir: &Path,
) -> Result<(), Box<dyn Error>> {
    unsafe {
        // SAFETY: `set_var` is only unsound while other threads touch the
        // environment. Cargo runs a build script in its own single-threaded
        // process, which is the only supported caller (see `generate_protos`).
        env::set_var("PROTOC", protoc_bin_path()?);
    }

    let mut prost_config = tonic_prost_build::Config::new();
    prost_config.prost_path(PROST_PATH);

    config.builder.out_dir(out_dir).compile_with_config(
        prost_config,
        proto_files,
        &[parent_dir.to_string()],
    )?;

    rewrite::rewrite_generated_files(out_dir, packages)?;

    Ok(())
}
