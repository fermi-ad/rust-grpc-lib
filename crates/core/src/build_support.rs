//! Compiles the controls `.proto` files bundled with this crate into tonic and
//! prost code, from your `build.rs` (`build` feature, in
//! `[build-dependencies]`). The
//! [service template](https://github.com/fermi-ad/rust-grpc-service-template)
//! shows a complete setup.

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
/// Call it once from `build.rs`. It uses a vendored `protoc` and always
/// compiles all bundled protos; you cannot pick a subset or add your own.
/// That is deliberate: the protos come only from the org-wide
/// [interface-definitions](https://github.com/fermi-ad/interface-definitions)
/// repository, so every service and client shares one set of generated types.
/// For a new or changed RPC, change it there and run this repository's "Bump
/// Interface Definitions" workflow; once released, bump your `tag`. A version
/// bump can change generated types for protos you do not use; see the
/// CHANGELOG migration notes.
///
/// ```toml
/// [build-dependencies]
/// rust-grpc-lib = { git = "https://github.com/fermi-ad/rust-grpc-lib", tag = "vX.Y.Z", features = ["build"] }
/// ```
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
/// Include the file once, as a whole. The generated code names only
/// `::rust_grpc_lib`, so the crate needs no direct `tonic`, `tonic-prost` or
/// `prost` dependency:
///
/// ```text
/// mod proto { include!(concat!(env!("OUT_DIR"), "/proto.rs")); }
/// ```
///
/// A proto `package a.b;` becomes the module `proto::a::b`, with keyword
/// segments escaped (`third_party::r#type`). Well-known types are in
/// `google::protobuf`, and the `dpm` module is `#[deprecated]`.
///
/// # Errors
///
/// Fails if a bundled proto cannot be read or has no `package` line, if
/// `protoc` or tonic-prost-build fails, if a generated file cannot be
/// post-processed (see below), or if `OUT_DIR` is unset (outside a build
/// script) or `proto.rs` cannot be written.
///
/// # Generated paths
///
/// tonic-build writes `tonic::...` paths and cannot be told to use another
/// prefix. After compiling, this function therefore rewrites each path-start
/// `tonic::` in the generated `<package>.rs` files to
/// `::rust_grpc_lib::__private::tonic::`; the `prost` and `ProstCodec` paths
/// are configured to point there directly. The rewrite is lexical and touches
/// only paths that begin with `tonic::`.
///
/// # Environment
///
/// Reads `OUT_DIR` and overwrites `PROTOC` with the vendored binary, so call
/// it only from a single-threaded process such as a build script.
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

/// Generates the integration-test fixtures: compiles `proto_files` (with
/// `include_dir` as the include root) with the same configuration and
/// `tonic::` path rewrite as [`generate_protos`], writing the `<package>.rs`
/// files into `out_dir`, but no `proto.rs`.
///
/// Used by `scripts/gen-test-fixtures.sh`; not public API, and it can change
/// in any release. Like [`generate_protos`], it overwrites `PROTOC`, so call
/// it only from a single-threaded process.
///
/// # Errors
///
/// Fails if a proto cannot be read or has no `package` line, if `protoc` or
/// tonic-prost-build fails, or if a generated file cannot be post-processed.
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
/// Start from [`Config::new`] (or `Default`). Each method consumes and returns
/// the `Config`, so calls chain:
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
/// # Paths
///
/// Paths are **protobuf** names, not Rust paths; `"."` matches everything.
///
/// - The type and field methods take a package (`.common.alarm`), message
///   (`common.alarm.Status`), enum (`common.alarm.Status.Source`) or field
///   (`common.alarm.Status.device`). A leading `.` matches from the root;
///   without it, a path matches the end of a name.
/// - [`client_attribute`](Self::client_attribute) and
///   [`server_attribute`](Self::server_attribute) take `package.Service`
///   (`services.example.ExampleDB`); the `*_mod_attribute` methods take a
///   package (`services.example`). Neither takes a leading `.`.
pub struct Config {
    builder: Builder,
}

impl Config {
    /// Creates a `Config` with defaults that can be added to but not removed:
    /// Cargo reruns the build script when a bundled `.proto` changes, the
    /// well-known types are generated, and every client derives
    /// [`GrpcClient`](crate::GrpcClient), which generates the `public`
    /// (no token) and `authenticated` (token provider) constructors.
    #[must_use = "a `Config` does nothing until it is passed to `generate_protos`"]
    pub fn new() -> Self {
        let builder = tonic_prost_build::configure()
            .codec_path(CODEC_PATH)
            .emit_rerun_if_changed(true)
            .compile_well_known_types(true)
            .client_attribute(".", "#[derive(::rust_grpc_lib::GrpcClient)]");

        Config { builder }
    }

    /// Adds an attribute to the client struct (such as `ExampleDbClient<T>`)
    /// of each matching service.
    ///
    /// ```
    /// # use rust_grpc_lib::build_support::Config;
    /// let config = Config::new()
    ///     .client_attribute("services.example.ExampleDB", "#[derive(mockall::automock)]");
    /// ```
    #[must_use = "this consumes the `Config` and returns an updated one; the attribute is lost if the result is dropped"]
    pub fn client_attribute(self, path: &str, attribute: &str) -> Self {
        Config {
            builder: self.builder.client_attribute(path, attribute),
        }
    }

    /// Adds an attribute, such as a `#[cfg(...)]` gate, to the client modules
    /// (`*_client`) of each matching package.
    ///
    /// ```
    /// # use rust_grpc_lib::build_support::Config;
    /// let config = Config::new()
    ///     .client_mod_attribute("services.example", "#[cfg(feature = \"client\")]");
    /// ```
    #[must_use = "this consumes the `Config` and returns an updated one; the attribute is lost if the result is dropped"]
    pub fn client_mod_attribute(self, path: &str, attribute: &str) -> Self {
        Config {
            builder: self.builder.client_mod_attribute(path, attribute),
        }
    }

    /// Adds an attribute to matching enums, including `oneof` enums (matched
    /// as `package.Message.oneof_name`).
    ///
    /// ```
    /// # use rust_grpc_lib::build_support::Config;
    /// let config = Config::new()
    ///     .enum_attribute("common.alarm.Status.Severity", "#[derive(strum::Display)]");
    /// ```
    #[must_use = "this consumes the `Config` and returns an updated one; the attribute is lost if the result is dropped"]
    pub fn enum_attribute(self, path: &str, attribute: &str) -> Self {
        Config {
            builder: self.builder.enum_attribute(path, attribute),
        }
    }

    /// Adds an attribute to matching message fields, enum variants and `oneof`
    /// variants.
    ///
    /// ```
    /// # use rust_grpc_lib::build_support::Config;
    /// let config = Config::new()
    ///     .field_attribute("common.alarm.Status.device", "#[serde(skip_serializing)]");
    /// ```
    #[must_use = "this consumes the `Config` and returns an updated one; the attribute is lost if the result is dropped"]
    pub fn field_attribute(self, path: &str, attribute: &str) -> Self {
        Config {
            builder: self.builder.field_attribute(path, attribute),
        }
    }

    /// Adds an attribute to the structs of matching messages, not to enums.
    ///
    /// ```
    /// # use rust_grpc_lib::build_support::Config;
    /// let config = Config::new()
    ///     .message_attribute("services.example.IdList", "#[derive(SomeStructOnlyMacro)]");
    /// ```
    #[must_use = "this consumes the `Config` and returns an updated one; the attribute is lost if the result is dropped"]
    pub fn message_attribute(self, path: &str, attribute: &str) -> Self {
        Config {
            builder: self.builder.message_attribute(path, attribute),
        }
    }

    /// Adds an attribute to the server struct (such as `ExampleDbServer<T>`)
    /// of each matching service, but not to its trait.
    ///
    /// ```
    /// # use rust_grpc_lib::build_support::Config;
    /// let config = Config::new()
    ///     .server_attribute("services.example.ExampleDB", "#[custom_server_attr]");
    /// ```
    #[must_use = "this consumes the `Config` and returns an updated one; the attribute is lost if the result is dropped"]
    pub fn server_attribute(self, path: &str, attribute: &str) -> Self {
        Config {
            builder: self.builder.server_attribute(path, attribute),
        }
    }

    /// Adds an attribute, such as a lint override or `#[cfg(...)]` gate, to the
    /// server modules (`*_server`) of each matching package.
    ///
    /// ```
    /// # use rust_grpc_lib::build_support::Config;
    /// let config = Config::new()
    ///     .server_mod_attribute("services.example", "#[allow(clippy::too_many_arguments)]");
    /// ```
    #[must_use = "this consumes the `Config` and returns an updated one; the attribute is lost if the result is dropped"]
    pub fn server_mod_attribute(self, path: &str, attribute: &str) -> Self {
        Config {
            builder: self.builder.server_mod_attribute(path, attribute),
        }
    }

    /// Adds an attribute to matching messages and enums, including `oneof`
    /// enums; [`message_attribute`](Self::message_attribute) and
    /// [`enum_attribute`](Self::enum_attribute) target one kind only.
    ///
    /// ```
    /// # use rust_grpc_lib::build_support::Config;
    /// let config = Config::new()
    ///     .type_attribute(".", "#[derive(serde::Serialize, serde::Deserialize)]");
    /// ```
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
