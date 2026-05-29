# rust-grpc-lib

A Rust library that provides gRPC client connection pooling and build-time code-generation helpers for the Controls group's gRPC services.

## What it does

- Ships the `.proto` definitions from the [`interface-definitions`](https://github.com/fermi-ad/interface-definitions) repository so consumers can generate Rust types in their own `build.rs`
- Provides [`build_support::generate_protos`] to drive `tonic`/`prost` code generation from those bundled protos
- Manages a process-wide pool of lazily-connected `Channel`s so callers share connections without coordinating themselves

Code generation is the **consumer's responsibility**: you call `build_support::generate_protos()` from your own `build.rs`, which writes the generated Rust source into your crate's `OUT_DIR`. The generated types are then available wherever you include the output file.

## Adding as a dependency

This library is still internal to the Fermi-AD Github org. While we wait for approval to make it public, **only other internal projects will be able to use it**. Add it to your `Cargo.toml` like so:

```toml
[dependencies]
rust-grpc-lib = { git = "https://github.com/fermi-ad/rust-grpc-lib", tag = "vX.Y.Z", features = ["build"] }
```

The `build` feature enables the [`build_support`] module and its code-generation helpers. It is only needed at build time, so you can also declare it as a build dependency if you prefer to keep it out of your runtime dependency tree:

```toml
[dependencies]
rust-grpc-lib = { git = "https://github.com/fermi-ad/rust-grpc-lib", tag = "vX.Y.Z" }

[build-dependencies]
rust-grpc-lib = { git = "https://github.com/fermi-ad/rust-grpc-lib", tag = "vX.Y.Z", features = ["build"] }
```

## Setting up code generation

Create a `build.rs` at the root of your crate:

```rust
use rust_grpc_lib::build_support::{ Config, generate_protos };

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::new(); // Optionally, chain calls here for adding custom attributes.
    // Or, make `config` mutable and add attributes line-by-line. E.g., config = config.type_attribute(...);
    generate_protos(config)?;
    Ok(())
}
```

Then include the generated file somewhere in your crate (e.g. `src/proto.rs` or inline in `src/lib.rs`):

```rust
// src/proto.rs  (or wherever you want the module to live)
include!(concat!(env!("OUT_DIR"), "/proto.rs"));
```

After that, all generated message types and service clients are accessible through that module:

```rust
mod proto {
    include!(concat!(env!("OUT_DIR"), "/proto.rs"));
}

use proto::services::alarm_commands::alarm_commands_client::AlarmCommandsClient;
```

## Runtime requirement

This library requires a [Tokio](https://tokio.rs) runtime. Tonic's transport layer is built on Tokio and there is no way to use it without one.

## Getting a client

Call `pool::get` with the desired client type and endpoint:

```rust
use rust_grpc_lib::pool;

// `proto` is the module you set up above with include!(concat!(env!("OUT_DIR"), "/proto.rs"))
use crate::proto::services::alarm_commands::alarm_commands_client::AlarmCommandsClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let client: AlarmCommandsClient<_> = pool::get("http://alarm-commands-host:50051")?;
    // use client...
    Ok(())
}
```

`pool::get` returns a client backed by a shared, lazily-connected channel. Calling the function multiple times with the same endpoint string reuses the same underlying connection.

## Available services

All `.proto` definition files in the [`interface-definitions`](https://github.com/fermi-ad/interface-definitions) repository are bundled here. 
This library is a defacto version control for gRPC in our Rust projects, as each version of this library will pin a specific set of definitions. 
Updates to `interface-definitions` will require a new version of this library to be used in downstream applications.

The `google::protobuf` well-known types (`Timestamp`, `Duration`, `Any`, etc.) are also always generated.

## Adding `derive` macros to generated types (optional)

If you need the generated structs to implement additional traits (e.g. `serde::Serialize`), set the relevant configuration options before building:

```rust
// build.rs
use rust_grpc_lib::build_support::{ Config, generate_protos };

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut config = Config::new();
    config = config.type_attribute(".some.package", "#[derive(serde::Serialize)]");
    generate_protos(config)?;
    Ok(())
}
```

## Keepalive configuration

HTTP/2 keepalive probes are sent on every channel. The timing can be overridden at runtime via environment variables:

| Variable | Default | Meaning |
| -------- | ------- | ------- |
| `RUST_GRPC_LIB_KEEP_ALIVE_INTERVAL_SECS` | `30` | Seconds between keepalive pings |
| `RUST_GRPC_LIB_KEEP_ALIVE_TIMEOUT_SECS` | `10` | Seconds to wait for a keepalive ping acknowledgement before closing the connection |

Keepalive pings are also sent while the connection is idle.

## Development

This project uses a devcontainer. Open in VS Code with the Dev Containers extension installed and you will be prompted to reopen in the container, which has all required tools pre-installed.

```
# After cloning
git submodule update --init --recursive
cargo build
```

### Repository layout

This repository is a Cargo workspace. The library code that consumers depend on lives in [`crates/core/`](crates/core/). A companion proc-macro crate lives in [`crates/grpc_client_macro/`](crates/grpc_client_macro/); it is re-exported from the main crate as `rust_grpc_lib::GrpcClient`, so consumers never need to add it as a direct dependency.
