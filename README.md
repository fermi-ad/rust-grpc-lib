# rust-grpc-lib

Authenticated gRPC for Fermilab controls services, built on [tonic](https://docs.rs/tonic) 0.14 and imported as `rust_grpc_lib`. It is for teams writing Rust gRPC services and clients on the controls platform: each service method declares who may call it, and a gate enforces that before any handler runs.

## Install

The crate is distributed by git tag, not crates.io, and needs Rust 1.88+ (edition 2024, let-chains). No feature is on by default.

```toml
[dependencies]
# `jwks-url` lets the validator fetch keys from AUTH_JWKS_URL; drop it if you load keys from a file.
rust-grpc-lib = { git = "https://github.com/fermi-ad/rust-grpc-lib", tag = "v8.0.0", features = ["server", "jwks-url"] }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
tracing = "0.1"
tracing-subscriber = "0.3"

[build-dependencies]
rust-grpc-lib = { git = "https://github.com/fermi-ad/rust-grpc-lib", tag = "v8.0.0", features = ["build"] }

[dev-dependencies]
rust-grpc-lib = { git = "https://github.com/fermi-ad/rust-grpc-lib", tag = "v8.0.0", features = ["test-util"] }
```

A client-only crate drops `server` and `jwks-url` and needs just `rust-grpc-lib` and `tokio`. Do not add `tonic`, `prost` or `async-trait` and do not rename the dependency; use the re-exports `rust_grpc_lib::tonic`, `rust_grpc_lib::prost` and `#[rust_grpc_lib::async_trait]`. `test-util` ships a publicly known key: dev-dependencies only. Details: [Adding the dependency](docs/server.md#adding-the-dependency).

## Quick start

Generate the bundled protos in `build.rs` (no system `protoc` needed):

```rust
// build.rs
use rust_grpc_lib::build_support::{Config, generate_protos};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    generate_protos(Config::new())
}
```

Define a service. Every method declares exactly one policy: `#[roles(..)]`, `#[any_authenticated]` or `#[public]`.

```rust
// src/main.rs
mod proto {
    include!(concat!(env!("OUT_DIR"), "/proto.rs"));
}

use proto::services::example::{
    IdList, ReplyOne, ReplyTwo,
    example_db_server::{ExampleDb, ExampleDbServer},
};
use rust_grpc_lib::access_controlled_service;
use rust_grpc_lib::auth::{KeyValidator, KeyValidatorConfig, AuthenticatedContext};
use rust_grpc_lib::server::GrpcServer;
use rust_grpc_lib::tonic::{Response, Status};
use std::sync::Arc;

struct ExampleDbService;

#[access_controlled_service] // must be ABOVE #[rust_grpc_lib::async_trait]
#[rust_grpc_lib::async_trait]
impl ExampleDb for ExampleDbService {
    #[roles(any("operator", "admin"))]
    async fn get_end_point_one(&self, ctx: AuthenticatedContext<IdList>) -> Result<Response<ReplyOne>, Status> {
        tracing::info!(user = ctx.sub(), ids = ctx.message.id.len(), "getEndPointOne");
        Ok(Response::new(ReplyOne::default()))
    }

    #[any_authenticated]
    async fn get_end_point_two(&self, _ctx: AuthenticatedContext<IdList>) -> Result<Response<ReplyTwo>, Status> {
        Ok(Response::new(ReplyTwo::default()))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init(); // the library never installs a subscriber

    // Key source from the environment: AUTH_JWKS_URL (needs the `jwks-url` feature),
    // AUTH_JWKS_FILE or AUTH_PEM_FILE. See docs/server.md#environment-variables.
    let validator = Arc::new(KeyValidator::new(KeyValidatorConfig::from_env()?)?);

    GrpcServer::builder(validator)
        .add_service(ExampleDbServer::new, ExampleDbService)
        .serve("0.0.0.0:50051".parse()?) // drains on SIGTERM/SIGINT
        .await?;
    Ok(())
}
```

Call it from a client: `authenticated` attaches a token to every request, `public` sends none (for `#[public]` methods).

```rust
use proto::services::example::example_db_client::ExampleDbClient;
use rust_grpc_lib::auth::FileTokenProvider;

let mut client = ExampleDbClient::authenticated(
    "http://localhost:50051",
    FileTokenProvider::from_env()?, // reads SERVICE_TOKEN_FILE
)?;
let reply = client.get_end_point_two(IdList::default()).await?;
```

For a complete, runnable service, start from the [service template](https://github.com/fermi-ad/rust-grpc-service-template).

## What you get

- **Access gate:** per-method policies enforced before any handler, with no switch to turn auth off. [Server guide](docs/server.md#writing-an-access-controlled-service)
- **Server:** `GrpcServer` with open health checks, RPC timeouts and signal-driven graceful shutdown. [Building and serving](docs/server.md#building-and-serving), [Graceful shutdown](docs/server.md#graceful-shutdown)
- **Telemetry:** per-RPC `tracing` spans, and call metrics exported over OTLP/HTTP when an endpoint is configured. [Telemetry](docs/server.md#telemetry), [Tracing and logs](docs/server.md#tracing-and-logs)
- **Clients:** generated `public` and `authenticated` constructors on a shared, lazily connected channel pool, plus token forwarding between services. [Client guide](docs/client.md)
- **Testing helpers:** token minting, handler request builders, a loopback `TestServer`, health, conformance, span and in-memory metric checks. [Testing guide](docs/testing.md)
- **Build-time proto generation:** every bundled proto is compiled by `generate_protos`. Protos come only from [interface-definitions](https://github.com/fermi-ad/interface-definitions); to add or change an RPC, change it there and run the bump workflow below. [Generating the protos](docs/server.md#generating-the-protos)

## Documentation

- [Concepts](docs/concepts.md): how the pieces fit, and a glossary. Start here if tonic, JWTs or Keycloak are new to you.
- [Server](docs/server.md), [Client](docs/client.md), [Testing](docs/testing.md) and [Troubleshooting](docs/troubleshooting.md).
- [API docs](https://fermi-ad.github.io/rust-grpc-lib/rust_grpc_lib/index.html) for every item; the [feature table](docs/server.md#adding-the-dependency) is in the Server guide.
- [CHANGELOG](CHANGELOG.md): release notes and migration guides.

## Contributing

```bash
git submodule update --init --recursive   # needed in a clone of this repo; consumers do not need it
cargo test --workspace --all-features
TRYBUILD=overwrite cargo test -p integration_tests compile_fail   # regenerate compile-fail expectations, then review the diff
bash scripts/gen-test-fixtures.sh         # regenerate crates/integration_tests/src/fixtures/ after a submodule update
```

- Bump the interface definitions with the **"Bump Interface Definitions"** GitHub workflow rather than by editing the submodule by hand. Anyone who uses this library can run it: it takes `minor` or `major` and a change description, and opens a PR. To do it locally, run `./scripts/update-interface-definitions.sh --minor|--major --description "..."`.
- Releases are tagged `vX.Y.Z`, matching the version in `crates/core/Cargo.toml`.

## License

Apache-2.0. See [`LICENSE`](LICENSE).
