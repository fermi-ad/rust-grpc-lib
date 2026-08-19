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
use rust_grpc_lib::{pool, auth::FileTokenProvider};

// `proto` is the module you set up above with include!(concat!(env!("OUT_DIR"), "/proto.rs"))
use crate::proto::services::alarm_commands::alarm_commands_client::AlarmCommandsClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let provider = FileTokenProvider::from_env()?;
    let client: AlarmCommandsClient<_> = pool::get("http://alarm-commands-host:50051", provider)?;
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
git submodule update --init --recursive --remote
cargo build
```

### Repository layout

This repository is a Cargo workspace. The library code that consumers depend on lives in [`crates/core/`](crates/core/). A companion proc-macro crate lives in [`crates/grpc_macro/`](crates/grpc_macro/); its public contents are re-exported from the main crate so consumers never need to add it as a direct dependency.

### Integration test fixtures

The integration tests in [`crates/core/tests/integration_round_trip.rs`](crates/core/tests/integration_round_trip.rs) exercise the full client→server gRPC round-trip with real JWT authentication. They use pre-generated Rust source files committed to [`crates/core/tests/fixtures/`](crates/core/tests/fixtures/) so that `cargo test` requires no `build.rs` or live `protoc` invocation.

**If the `interface-definitions` submodule is updated**, regenerate the fixtures by running:

```bash
# Pull the latest submodule changes first
git submodule update --remote

# Regenerate the committed fixture files
bash scripts/gen-test-fixtures.sh

# Commit the updated fixtures alongside the submodule bump
git add crates/core/tests/fixtures/ crates/core/interface-definitions
git commit -m "chore: regenerate test fixtures for updated interface-definitions"
```

The script compiles `DevDB.proto` (a representative service with a simple unary RPC) using the vendored `protoc` binary — no system-level protobuf installation is required.

## Authentication (Zero-Trust JWT)

The `auth` feature (enabled by default) wires zero-trust JWT authentication into every gRPC client and server. All outbound calls carry a `Bearer` token; all inbound calls are validated before reaching your handler.

### Middle-layer service (most common)

A service that sits between the edge and the GraphQL gateway. It validates the incoming user JWT, enforces role-based access control via `#[roles(...)]`, and forwards the same token downstream to other gRPC services.

```rust
use std::sync::Arc;
use rust_grpc_lib::auth::{
    KeycloakClaims, StaticKeysValidator, StaticKeysValidatorConfig,
    JwtValidationLayer, ForwardedTokenInterceptor,
};

struct MyDaqService;

#[rust_grpc_lib::grpc_service]
impl Daq for MyDaqService {
    #[roles(any("viewer", "operator", "admin"))]
    async fn get_data(&self, req: Request<GetDataRequest>) -> Result<Response<GetDataResponse>, Status> {
        // Forward the upstream token to a downstream service
        let client: AlarmCommandsClient<_> = rust_grpc_lib::pool::get(
            "http://alarm-host:50051",
            ForwardedTokenInterceptor::from_request(&req),
        )?;
        todo!()
    }

    #[roles(any("operator", "admin"))]
    async fn set_data(&self, req: Request<SetDataRequest>) -> Result<Response<SetDataResponse>, Status> {
        todo!()
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Reads AUTH_JWKS_FILE or AUTH_PEM_FILE; optionally AUTH_ISSUER
    let validator = Arc::new(StaticKeysValidator::new(StaticKeysValidatorConfig::from_env()?)?);

    tonic::transport::Server::builder()
        .layer(JwtValidationLayer::new(validator))
        .add_service(DaqServer::new(MyDaqService))
        .serve("[::1]:50051".parse()?)
        .await?;

    Ok(())
}
```

### Edge/hardware service

A service that runs close to hardware and pushes data upstream. It authenticates itself using a platform-injected token file (e.g. a Vault or Kubernetes secret) rather than forwarding a user JWT.

```rust
use rust_grpc_lib::auth::FileTokenProvider;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Reads SERVICE_TOKEN_FILE (path to Vault/K8s-injected token file).
    // Caches the token for SERVICE_TOKEN_CACHE_TTL_SECS (default: 30 s).
    let provider = FileTokenProvider::from_env()?;

    let client: DaqClient<_> = rust_grpc_lib::pool::get(
        "http://daq-host:50051",
        provider,
    )?;

    loop {
        client.send_hardware_data(/* ... */).await?;
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}
```

### GraphQL gateway

The gateway sits at the user-facing edge of the system. It:

- Installs `JwtValidationLayer` on its tonic server to validate incoming user JWTs before any handler is reached.
- Uses `ForwardedTokenInterceptor::from_request(&req)` when calling downstream gRPC services, so the validated user JWT is propagated through the entire call chain without re-issuing tokens.
- Reads its own service JWT from a platform-injected file via `FileTokenProvider::from_env()` for any service-to-service calls that require a service identity (e.g. publishing to Kafka).

### Test harness

Disable auth entirely with the `unauthenticated` feature. `pool::get_unauthenticated` is then available and requires no token provider.

```toml
# Cargo.toml
rust-grpc-lib = { git = "...", tag = "v4.0.0", default-features = false, features = ["unauthenticated"] }
```

```rust
// No provider needed — pool::get_unauthenticated is available
let client: DaqClient<_> = rust_grpc_lib::pool::get_unauthenticated("http://localhost:50051")?;
```

## Feature Flags

| Feature | Default | Description |
|---|---|---|
| `auth` | ✅ on | Enables JWT auth; requires `pool::get` to accept a `TokenProvider` |
| `jwks-url` | off | Enables live JWKS endpoint rotation via `JwksValidator` (opt-in) |
| `unauthenticated` | off | Enables `pool::get_unauthenticated` with no auth; for test harnesses only |
| `build` | off | Enables proto code-generation helpers (`build_support` module) |

## Environment Variables

All auth-related environment variables recognized by this library:

| Variable | Default | Description |
|---|---|---|
| `AUTH_JWKS_FILE` | — | Path to a local JWKS JSON file used by `StaticKeysValidator` |
| `AUTH_PEM_FILE` | — | Path to a PEM-encoded RSA public key used by `StaticKeysValidator` (alternative to `AUTH_JWKS_FILE`) |
| `AUTH_ISSUER` | — | Expected `iss` claim value; omit to skip issuer validation |
| `AUTH_JWKS_URL` | — | Live JWKS endpoint URL used by `JwksValidator` (requires `jwks-url` feature) |
| `AUTH_JWKS_CACHE_TTL_SECS` | `300` | How long `JwksValidator` caches the fetched key set before re-fetching |
| `SERVICE_TOKEN_FILE` | — | Path to a Vault- or Kubernetes-injected service token file used by `FileTokenProvider` |
| `SERVICE_TOKEN_CACHE_TTL_SECS` | `30` | How long `FileTokenProvider` caches the token before re-reading the file |
| `RUST_GRPC_LIB_KEEP_ALIVE_INTERVAL_SECS` | `30` | Seconds between HTTP/2 keepalive pings on every pooled channel |
| `RUST_GRPC_LIB_KEEP_ALIVE_TIMEOUT_SECS` | `10` | Seconds to wait for a keepalive ping acknowledgement before closing the connection |
