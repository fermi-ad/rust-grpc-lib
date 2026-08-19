# rust-grpc-lib

The latest documentation for this crate [can be found here](https://fuzzy-adventure-5v1nq96.pages.github.io/rust_grpc_lib/index.html)

A Rust library for building gRPC services in the Controls group. It handles three things so you don't have to:

1. **Proto bundling & code generation** — the `.proto` definitions from [`interface-definitions`](https://github.com/fermi-ad/interface-definitions) are shipped with this library. Call `build_support::generate_protos()` from your `build.rs` and you get fully-typed Rust message and client structs with no manual proto management.
2. **Connection pooling** — a process-wide pool of lazily-connected channels, keyed by endpoint string. Generated client constructors share connections automatically; no manual pool management is required.
3. **Zero-trust JWT auth** — outbound calls carry a `Bearer` token; inbound calls are validated before reaching your handler. Role-based access control is enforced via the `#[keycloak_authenticated_service]` / `#[roles(...)]` proc-macro attributes.

> **Tokio required.** This library depends on Tonic, which requires a Tokio async runtime. All examples below assume you are inside `#[tokio::main]` or an equivalent async context.

---

## Quick start

### 1. Add the dependency

This library is internal to the Fermi-AD GitHub org. Add it to your `Cargo.toml`:

```toml
[dependencies]
rust-grpc-lib = { git = "https://github.com/fermi-ad/rust-grpc-lib", tag = "vX.Y.Z" }

[build-dependencies]
rust-grpc-lib = { git = "https://github.com/fermi-ad/rust-grpc-lib", tag = "vX.Y.Z", features = ["build"] }
```

The `build` feature enables the `build_support` module and its code-generation helpers. Declaring it only under `[build-dependencies]` keeps it out of your runtime binary.

If you only need code generation and don't care about separating build vs. runtime deps, you can add it once under `[dependencies]` with `features = ["build"]`.

### 2. Generate Rust types from the bundled protos

Create a `build.rs` at the root of your crate:

```rust
use rust_grpc_lib::build_support::{Config, generate_protos};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    generate_protos(Config::new())?;
    Ok(())
}
```

This writes a single file, `proto.rs`, into your crate's `OUT_DIR`.

### 3. Include the generated module

Add this wherever you want the generated types to live — a dedicated `src/proto.rs` is the most common choice:

```rust
// src/proto.rs
include!(concat!(env!("OUT_DIR"), "/proto.rs"));
```

Then expose it from your crate root:

```rust
// src/lib.rs  (or src/main.rs)
mod proto;
```

All generated message types and service clients are now accessible through `proto::`:

```rust
use crate::proto::services::alarm_commands::alarm_commands_client::AlarmCommandsClient;
```

The `google::protobuf` well-known types (`Timestamp`, `Duration`, `Any`, etc.) are also always generated.

### 4. Get a client

```rust
use rust_grpc_lib::auth::FileTokenProvider;
use crate::proto::services::alarm_commands::alarm_commands_client::AlarmCommandsClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Reads SERVICE_TOKEN_FILE from the environment (see Environment Variables below).
    let provider = FileTokenProvider::from_env()?;

    let client = AlarmCommandsClient::from_endpoint_with_provider(
        "http://alarm-commands-host:50051",
        provider,
    )?;

    // use client...
    Ok(())
}
```

`from_endpoint_with_provider` is generated on every client struct by `#[derive(GrpcClient)]`, which `Config::new()` applies automatically. It returns a client backed by a shared, lazily-connected channel. Calling it multiple times with the same endpoint string reuses the same underlying connection.

<<<<<<< HEAD
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
=======
---
>>>>>>> c92f405 (enhance readme clarity)

## Authentication (Zero-Trust JWT)

The `auth` feature is **enabled by default**. All outbound calls carry a `Bearer` token; all inbound calls are validated before reaching your handler.

There are three common service archetypes. Pick the one that matches your service's role in the system.

### Middle-layer service

Sits between the edge and the GraphQL gateway. Validates the incoming user JWT, enforces role-based access control, and forwards the same token downstream.

```rust
use std::sync::Arc;
use rust_grpc_lib::auth::{
    StaticKeysValidator, StaticKeysValidatorConfig,
    validator_into_layer, extract_token,
};

struct MyDaqService;

#[rust_grpc_lib::keycloak_authenticated_service]
impl Daq for MyDaqService {
    // At least one of the listed roles must be present in the JWT.
    #[roles(any("viewer", "operator", "admin"))]
    async fn get_data(
        &self,
        req: Request<GetDataRequest>,
    ) -> Result<Response<GetDataResponse>, Status> {
        // Forward the caller's token to a downstream service.
        // extract_token pulls the Bearer token out of the incoming request
        // and wraps it as a ForwardedToken, which implements TokenProvider.
        let provider = extract_token(&req)?;
        let client = AlarmCommandsClient::from_endpoint_with_provider(
            "http://alarm-host:50051",
            provider,
        )?;
        todo!()
    }

    // Every listed role must be present.
    #[roles(all("operator", "admin"))]
    async fn set_data(
        &self,
        req: Request<SetDataRequest>,
    ) -> Result<Response<SetDataResponse>, Status> {
        todo!()
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Reads AUTH_JWKS_FILE or AUTH_PEM_FILE; optionally AUTH_ISSUER.
    let validator = Arc::new(
        StaticKeysValidator::new(StaticKeysValidatorConfig::from_env()?)?
    );

    tonic::transport::Server::builder()
        .layer(validator_into_layer(validator))
        .add_service(DaqServer::new(MyDaqService))
        .serve("[::1]:50051".parse()?)
        .await?;

    Ok(())
}
```

### Edge/hardware service

Runs close to hardware and pushes data upstream. Authenticates itself using a platform-injected token file (e.g. a Vault or Kubernetes secret).

```rust
use rust_grpc_lib::auth::FileTokenProvider;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Reads SERVICE_TOKEN_FILE; caches for SERVICE_TOKEN_CACHE_TTL_SECS (default: 30 s).
    let provider = FileTokenProvider::from_env()?;

    let client = DaqClient::from_endpoint_with_provider("http://daq-host:50051", provider)?;

    loop {
        client.send_hardware_data(/* ... */).await?;
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}
```

### GraphQL gateway

The gateway sits at the user-facing edge of the system. It:

- Installs `validator_into_layer(validator)` on its Tonic server to validate incoming user JWTs before any handler is reached.
- Uses `extract_token(&req)` to pull the validated user JWT out of an incoming request, then passes the resulting `ForwardedToken` to `ClientName::from_endpoint_with_provider` when calling downstream gRPC services, so the user identity propagates through the entire call chain without re-issuing tokens.
- Reads its own service JWT from a platform-injected file via `FileTokenProvider::from_env()` for any service-to-service calls that require a service identity (e.g. publishing to Kafka).

### Disabling auth (test harnesses only)

Use the `unauthenticated` feature to bypass auth entirely. When enabled, every generated client struct gains a `from_endpoint` constructor that requires no token provider. **Do not use in production.**

```toml
# Cargo.toml
rust-grpc-lib = { git = "...", tag = "vX.Y.Z", default-features = false, features = ["unauthenticated"] }
```

```rust
let client = DaqClient::from_endpoint("http://localhost:50051")?;
```

---

## Customizing generated code

### Adding attributes to generated types

If you need generated structs to implement additional traits (e.g. `serde::Serialize`), configure `Config` before calling `generate_protos`:

```rust
// build.rs
use rust_grpc_lib::build_support::{Config, generate_protos};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::new()
        // Add an attribute to every type in a proto package.
        .type_attribute(".controls.common.v1", "#[derive(serde::Serialize, serde::Deserialize)]")
        // Add an attribute only to generated client types.
        .client_attribute(".controls.service.daq.v1", "#[derive(my_client_macro)]");

    generate_protos(config)?;
    Ok(())
}
```

The proto package path (first argument) matches the `package` declaration in the `.proto` file. Use `.` as a prefix (e.g. `.controls.common.v1`).

### Wrapping a generated client in a newtype

`Config::new()` automatically applies `#[derive(GrpcClient)]` to every generated client struct, so you get `from_endpoint_with_provider` for free on all generated types. If you wrap a generated client in your own newtype, apply the derive to your wrapper so it also gains `from_endpoint_with_provider`:

```rust
use rust_grpc_lib::GrpcClient;

#[derive(GrpcClient)]
pub struct MyAlarmClient<T>(alarm_commands_client::AlarmCommandsClient<T>);
```

---

## Proc-macro reference

### `#[derive(GrpcClient)]`

Applied automatically by `Config::new()` to every generated client struct. Generates a `from_endpoint_with_provider(endpoint, provider)` constructor that retrieves a pooled channel and wraps it with `ClientJwtInterceptor`. Requires the `auth` feature (on by default).

### `#[derive(GrpcNoAuthClient)]`

Generates a `from_endpoint(endpoint)` constructor with no auth wiring. Applied automatically by `Config::new()` when the `unauthenticated` feature is enabled. Can also be applied manually to custom wrapper types. **Do not use in production.**

### `#[keycloak_authenticated_service]`

Applied to an `impl Trait for Type` block. Injects Keycloak role-checking guards into methods annotated with `#[roles(...)]`. Methods without `#[roles(...)]` are left untouched — a valid JWT is still required by `JwtValidationLayer`, but no role check is injected by this macro.

### `#[roles(...)]`

Marker attribute consumed by `#[keycloak_authenticated_service]`. Two variants:

| Variant | Meaning |
|---|---|
| `#[roles(any("r1", "r2"))]` | At least one of the listed roles must be present in the JWT claims |
| `#[roles(all("r1", "r2"))]` | Every listed role must be present |

---

## Reference

### Feature flags

| Feature | Default | Description |
|---|---|---|
| `auth` | ✅ on | Enables JWT auth; generated clients gain `from_endpoint_with_provider` |
| `jwks-url` | off | Enables live JWKS endpoint rotation via `JwksValidator` (opt-in; pulls in an HTTP client) |
| `unauthenticated` | off | Enables `from_endpoint` (no-auth constructor) on generated clients; for test harnesses only |
| `build` | off | Enables proto code-generation helpers (`build_support` module) |

### Environment variables

Check [`rust-auth-lib`'s README](https://github.com/fermi-ad/rust-auth-lib) for the list of environment variables that are expected by the re-exported elements in this library.

| Variable | Default | Description |
|---|---|---|
| `RUST_GRPC_LIB_KEEP_ALIVE_INTERVAL_SECS` | `30` | Seconds between HTTP/2 keepalive pings on every pooled channel |
| `RUST_GRPC_LIB_KEEP_ALIVE_TIMEOUT_SECS` | `10` | Seconds to wait for a keepalive ping acknowledgement before closing the connection |

Keepalive pings are sent on every channel, including while the connection is idle.

### Available proto definitions

All `.proto` files from the [`interface-definitions`](https://github.com/fermi-ad/interface-definitions) submodule are bundled with this library. Each version of `rust-grpc-lib` pins a specific revision of `interface-definitions`, making this library the de-facto version control for gRPC definitions in our Rust projects. Updating `interface-definitions` requires a new version of this library.

---

## Development

This project uses a devcontainer. Open it in VS Code with the Dev Containers extension installed and you will be prompted to reopen in the container, which has all required tools pre-installed.

```bash
# After cloning
git submodule update --init --recursive
cargo build

# Run all tests (unit + integration)
cargo test
```

### Repository layout

This is a Cargo workspace with three crates:

| Crate | Path | Description |
|---|---|---|
| `rust-grpc-lib` | [`crates/core/`](crates/core/) | The main library crate consumers depend on. Contains the connection pool, auth wiring, and build-support helpers. |
| `grpc-macro` | [`crates/grpc_macro/`](crates/grpc_macro/) | Proc-macro crate providing `#[derive(GrpcClient)]`, `#[derive(GrpcNoAuthClient)]`, `#[keycloak_authenticated_service]`, and `#[roles(...)]`. Re-exported from the main crate — consumers never need to depend on this directly. |
| `integration_tests` | [`crates/integration_tests/`](crates/integration_tests/) | Integration-test crate. Exercises the full client→server gRPC round-trip with real JWT auth and tests the `#[keycloak_authenticated_service]` macro expansion. |

### Integration test fixtures

The integration tests use pre-generated Rust source files committed to [`crates/integration_tests/src/fixtures/`](crates/integration_tests/src/fixtures/) so that `cargo test` requires no `build.rs` or live `protoc` invocation.

**If the `interface-definitions` submodule is updated**, regenerate the fixtures:

```bash
# Pull the latest submodule changes
git submodule update --remote

# Regenerate the committed fixture files
bash scripts/gen-test-fixtures.sh

# Commit the updated fixtures alongside the submodule bump
git add crates/integration_tests/src/fixtures/ crates/core/interface-definitions
git commit -m "chore: regenerate test fixtures for updated interface-definitions"
```

The script compiles `DevDB.proto` using the vendored `protoc` binary — no system-level protobuf installation is required.
