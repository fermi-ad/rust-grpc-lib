//! Build-time helpers for compiling the bundled `.proto` definitions into Rust.
//!
//! The public entry point is [`generate_protos`], which consumers call from
//! their own `build.rs`.

use std::{env, error::Error, ffi::OsStr, fs, path::Path};

mod file_utils;
mod proto_rs;

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
/// use rust_grpc_lib::build_support::{ Config, generate_protos };
///
/// fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let mut config = Config::new();
///     // ... configure custom attributes for the generated code here
///     // e.g., config = config.type_attribute(".some.package", "#[derive(my_custom_attr)]");
///     generate_protos(config)?;
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
/// # Customizing generated attributes
///
/// The provided [`Config`] struct exposes functions to add custom attributes to the
/// generated code.
///
/// Example — add `serde` derives to every type in the `common.alarm` package:
///
/// ```rust,ignore
/// let mut config = Config::new();
/// config = config.type_attribute(".common.alarm", "#[derive(serde::Serialize, serde::Deserialize)]");
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
/// - `tonic-prost-build` fails to compile the protos, or
/// - `OUT_DIR` is not set (i.e. the function is called outside of a build script)
pub fn generate_protos(config: Config) -> Result<(), Box<dyn Error>> {
    // `env!("CARGO_MANIFEST_DIR")` is resolved at compile time of *this* crate
    // and baked into the binary. Because the `.proto` files are included in the
    // published crate via the `include` field in Cargo.toml, this path is valid
    // on the consumer's machine even though it points into this crate's source.
    let interface_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/interface-definitions");
    let mut proto_dir = interface_dir.to_string();
    proto_dir.push_str("/proto");
    let proto_files = file_utils::find_proto_files(&proto_dir, OsStr::new("proto"))?;

    compile(interface_dir, &proto_files, config)?;

    let packages = file_utils::collect_packages(&proto_files)?;
    let proto_rs_source = proto_rs::build_contents(&packages);

    let out_dir = env::var("OUT_DIR")?;
    let out_path = Path::new(&out_dir).join("proto.rs");
    fs::write(&out_path, proto_rs_source)?;

    Ok(())
}

/// Provides configuration options for the generated code.
///
/// Acquire an instance with the [`new`](Self::new) function, then add any attributes as needed
/// before passing to [`generate_protos`].
///
/// Each attribute function consumes the current instance of [`Config`] and returns an updated one.
/// Calls may be chained, like so:
///
/// ```rust,ignore
/// let config = Config::new()
///     .type_attribute("some.package", "#[my_attr]")
///     .message_attribute("some.package.MyMessage", "#[message_specific_attr]")
///     .server_mod_attribute("other.package", "#[server_module_attr]");
/// ```
///
/// Alternatively, declare your `config` variable as mutable and replace it with each call:
///
/// ```rust,ignore
/// let mut config = Config::new();
/// config = config.type_attribute("some.package", "#[my_attr]");
/// config = config.message_attribute("some.package.MyMessage", "#[message_specific_attr]");
/// // ...
/// ```
pub struct Config {
    builder: tonic_prost_build::Builder,
}

impl Config {
    pub fn new() -> Self {
        let mut builder = tonic_prost_build::configure()
            .emit_rerun_if_changed(true)
            .compile_well_known_types(true);

        if cfg!(any(feature = "auth", test)) {
            builder = builder.client_attribute(".", "#[derive(::rust_grpc_lib::GrpcClient)]");
        }

        if cfg!(any(feature = "unauthenticated", test)) {
            builder = builder.client_attribute(".", "#[derive(::rust_grpc_lib::GrpcNoAuthClient)]");
        }

        Config { builder }
    }

    /// Add an additional attribute to generated gRPC client service structs.
    ///
    /// # Differentiation from other attribute functions
    /// - **Scope:** gRPC-specific tooling. This targets the generated implementation client (e.g., `pub struct MyServiceClient<T>`).
    /// - **Vs `server_attribute`:** Modifies the outgoing client consumer types, leaving the server handler traits untouched.
    /// - **Vs `type_attribute` / `message_attribute`:** Data-layer configurations (`type_attribute`) target the underlying data shapes.
    ///   Service-layer configurations (`client_attribute`) target the communication infrastructure generated by `tonic`.
    /// - **Vs `client_mod_attribute`:** Attaches to the actual client code items, whereas `client_mod_attribute` wraps
    ///   the module container boundary.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use rust_grpc_lib::build_support::Config;
    ///
    /// let mut config = Config::new();
    /// // Attaches a mock trait derive directly onto the generated gRPC client struct
    /// config = config.client_attribute("my_package.MyService", "#[derive(mockall::automock)]");
    /// ```
    pub fn client_attribute(self, path: &str, attribute: &str) -> Self {
        Config {
            builder: self.builder.client_attribute(path, attribute),
        }
    }

    /// Add an additional attribute to the module namespace block containing the client stubs.
    ///
    /// # Differentiation from other attribute functions
    /// - **Scope:** Architectural boundary/Module encapsulation. Targets the generated `pub mod my_service_client` statement.
    /// - **Vs `client_attribute`:** `client_attribute` edits things *inside* the module (like the client struct itself).
    ///   `client_mod_attribute` gates or tags the *entire parent module*.
    /// - **Vs `server_mod_attribute`:** Isolates compilation properties strictly for your client implementations, ignoring server scopes.
    ///   This is commonly used for conditional compilation (e.g., `#[cfg(feature = "client")]`) so client modules don't compile
    ///   when the feature flag is missing.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use rust_grpc_lib::build_support::Config;
    ///
    /// let mut config = Config::new();
    /// // Conditionally compiles the entire client module structure using cargo feature gates
    /// config = config.client_mod_attribute("my_package.MyService", "#[cfg(feature = \"client\")]");
    /// ```
    pub fn client_mod_attribute(self, path: &str, attribute: &str) -> Self {
        Config {
            builder: self.builder.client_mod_attribute(path, attribute),
        }
    }

    /// Add an additional attribute to matched standalone enum definitions.
    ///
    /// # Differentiation from other attribute functions
    /// - **Scope:** Enums exclusively. It targets **only** Rust `enum` structures generated from formal,
    ///   standalone Protobuf `enum` blocks.
    /// - **Vs `type_attribute`:** `type_attribute` applies macros globally to both structs and enums.
    ///   `enum_attribute` filters out message structs, ensuring your macro only runs on actual enum choices.
    /// - **Vs `message_attribute`:** Exact opposites. `message_attribute` exclusively targets struct types,
    ///   while `enum_attribute` strictly targets enum types.
    /// - **The `oneof` Caveat:** Inside a generated `.rs` file, a Protobuf `oneof` block is compiled as a Rust `enum`
    ///   to wrap exclusive variant fields. However, `enum_attribute` does **not** catch these fields. To attach macros
    ///   to a `oneof` enum structure, you must explicitly use `type_attribute` paired with the exact field path.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use rust_grpc_lib::build_support::Config;
    ///
    /// let mut config = Config::new();
    /// // Derives enum-specific utilities (like string mapping) only on standalone enums
    /// config.enum_attribute("my_package.UserRole", "#[derive(strum::EnumString, strum::Display)]");
    /// ```
    pub fn enum_attribute(self, path: &str, attribute: &str) -> Self {
        Config {
            builder: self.builder.enum_attribute(path, attribute),
        }
    }

    /// Add an additional attribute to individual struct fields or enum variants.
    ///
    /// # Differentiation from other attribute functions
    /// - **Scope:** Sub-item block placement. It injects code **inside** the generated data types, directly above
    ///   individual struct fields or the variants inside a `oneof` enum block.
    /// - **Vs `type_attribute` / `message_attribute`:** Those methods append attributes to the top level of the
    ///   type declaration. `field_attribute` is used exclusively for property-level tuning, such as field skipping,
    ///   renaming, default values, or target serialization hooks.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use rust_grpc_lib::build_support::Config;
    ///
    /// let mut config = Config::new();
    /// // Injects a serde rule directly above the `hashed_password` struct field
    /// config = config.field_attribute("my_package.User.hashed_password", "#[serde(skip_serializing)]");
    /// ```
    pub fn field_attribute(self, path: &str, attribute: &str) -> Self {
        Config {
            builder: self.builder.field_attribute(path, attribute),
        }
    }

    /// Add an additional attribute to matched messages specifically.
    ///
    /// # Differentiation from other attribute functions
    /// - **Scope:** Strict struct-only type-level macro. It targets **only** Rust `struct` definitions generated
    ///   from Protobuf messages.
    /// - **Vs `type_attribute`:** It automatically ignores all `enum` items and `oneof` enums. This is useful
    ///   if you use a derive macro that works safely on structs but panics or fails when applied to enums.
    /// - **Vs `field_attribute`:** Modifies the top-level message definition item, not individual fields within it.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use rust_grpc_lib::build_support::Config;
    ///
    /// let mut config = Config::new();
    /// // Applies a struct-specific macro only to the "User" message struct, ignoring enums
    /// config = config.message_attribute("my_package.User", "#[derive(SomeStructOnlyMacro)]");
    /// ```
    pub fn message_attribute(self, path: &str, attribute: &str) -> Self {
        Config {
            builder: self.builder.message_attribute(path, attribute),
        }
    }

    /// Add an additional attribute to generated gRPC server trait implementations.
    ///
    /// # Differentiation from other attribute functions
    /// - **Scope:** gRPC-specific tooling. Targets the generated trait defined for the server interface (e.g., `pub trait MyService`)
    ///   and the generated service server dispatcher (`pub struct MyServiceServer<T>`).
    /// - **Vs `client_attribute`:** Modifies only the server side of the contract, ignoring the client dispatcher code block.
    /// - **Vs `server_mod_attribute`:** Attaches to specific inner server structs/traits, while `server_mod_attribute` applies
    ///   attributes to the outer enclosing module scope.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use rust_grpc_lib::build_support::Config;
    ///
    /// let mut config = Config::new();
    /// // Forces a specific custom handling or restriction macro directly onto the server trait
    /// config = config.server_attribute("my_package.MyService", "#[custom_server_gate]");
    /// ```
    pub fn server_attribute(self, path: &str, attribute: &str) -> Self {
        Config {
            builder: self.builder.server_attribute(path, attribute),
        }
    }

    /// Add an additional attribute to the module namespace block containing the server stubs.
    ///
    /// # Differentiation from other attribute functions
    /// - **Scope:** Architectural boundary/Module encapsulation. Targets the generated `pub mod my_service_server` statement.
    /// - **Vs `server_attribute`:** Modifies the parent module containing the server structures instead of modifying the internal server trait elements.
    /// - **Vs `client_mod_attribute`:** Isolates compilation traits specifically for the server environment.
    ///   This is typically used to append module-wide documentation rules, clippy lint overrides, or conditional features
    ///   (e.g., `#[cfg(feature = "server")]`) to avoid tracking server logic on clean client dependencies.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use rust_grpc_lib::build_support::Config;
    ///
    /// let mut config = Config::new();
    /// // Disables specific Clippy warnings across the entire generated server codebase module
    /// config = config.server_mod_attribute("my_package.MyService", "#[allow(clippy::too_many_arguments)]");
    /// ```
    pub fn server_mod_attribute(self, path: &str, attribute: &str) -> Self {
        Config {
            builder: self.builder.server_mod_attribute(path, attribute),
        }
    }

    /// Add an additional attribute to matched messages, enums, and `oneof` types.
    ///
    /// # Differentiation from other attribute functions
    /// - **Scope:** Broadest type-level macro. Applies to **both** `struct` definitions (generated from
    ///   Protobuf messages) and `enum` definitions (generated from standalone enums or `oneof` groupings).
    /// - **Vs `message_attribute`:** `type_attribute` modifies both structs and enums. Use `message_attribute`
    ///   if you want to target structs exclusively.
    /// - **Vs `field_attribute`:** Operates on the root container definition (`struct MyMessage`), whereas
    ///   `field_attribute` operates on properties inside the container (`pub my_field: String`).
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use rust_grpc_lib::build_support::Config;
    ///
    /// let mut config = Config::new();
    /// // Derives Serialize/Deserialize for EVERY message and enum under the package
    /// config = config.type_attribute(".", "#[derive(serde::Serialize, serde::Deserialize)]");
    /// ```
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

/// Invoke [`tonic_prost_build::Builder::compile_protos`] on the given proto files.
///
/// # Safety
///
/// This function calls [`std::env::set_var`] to point `PROTOC` at the vendored
/// binary. That is only safe when no other threads are reading the environment
/// concurrently. Cargo runs `build.rs` single-threaded, so this is safe in
/// normal usage.
fn compile(parent_dir: &str, proto_files: &[String], config: Config) -> Result<(), Box<dyn Error>> {
    unsafe {
        // SAFETY: build scripts run single-threaded; `set_var` is only unsafe
        // in multi-threaded contexts.
        env::set_var("PROTOC", protoc_bin_vendored::protoc_bin_path()?);
    }

    config
        .builder
        .compile_protos(proto_files, &[parent_dir.to_string()])?;

    Ok(())
}
