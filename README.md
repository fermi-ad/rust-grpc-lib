# rust-grpc-lib

Batteries-included gRPC for Fermilab controls services, built on [tonic](https://docs.rs/tonic) 0.14 and imported as `rust_grpc_lib`. You write access-controlled services whose Keycloak/JWT policies are enforced by a gate before any handler runs. You also get generated clients that share a pooled channel, bundled proto generation, per-RPC tracing, and test helpers.

API docs: <https://fermi-ad.github.io/rust-grpc-lib/rust_grpc_lib/index.html>

## Highlights

- **Per-method access policies:** `#[roles(..)]`, `#[any_authenticated]` or `#[public]` on every method, with no switch to turn auth off. See [Writing an access-controlled service](docs/server.md#writing-an-access-controlled-service).
- **Bundled protos:** every controls proto is generated from your `build.rs`, and no system `protoc` is needed. Protos come only from [interface-definitions](https://github.com/fermi-ad/interface-definitions), so the whole org shares one set of types; for a new RPC, change it there and run the bump workflow yourself. See [Generating the protos](docs/server.md#generating-the-protos).
- **Clients:** every generated client gets `public(endpoint)`, which sends no token, and `authenticated(endpoint, provider)`, which attaches a Bearer token to every request; both use lazy connections on pooled HTTP/2 channels. The target method's policy decides which to use, and the server's gate is the authority. TLS is handled by the infra team, so do not configure tonic's TLS settings. See [Generated clients](docs/client.md#generated-clients).
- **Server builder:** an open health service (built in; no `tonic-health` dependency), timeouts, graceful shutdown and per-RPC spans. See [Building and serving](docs/server.md#building-and-serving) and [Tracing and logs](docs/server.md#tracing-and-logs).
- **Minimal dependencies:** `tonic`, `prost` and the `async_trait` attribute are re-exported (`rust_grpc_lib::tonic`, `rust_grpc_lib::prost`, `#[rust_grpc_lib::async_trait]`), so you do not add them yourself. See [Adding the dependency](docs/server.md#adding-the-dependency).
- **Test helpers:** token minting, handler request builders, health queries, live-server conformance checks and span capture. See [Testing](docs/testing.md).

## Quick look

A trimmed sketch of an access-controlled service. For a complete, runnable service, see the [service template](https://github.com/fermi-ad/rust-grpc-service-template).

```rust
use rust_grpc_lib::access_controlled_service;
use rust_grpc_lib::auth::RequestContext;
use rust_grpc_lib::tonic::{Response, Status};

#[access_controlled_service] // must be ABOVE #[rust_grpc_lib::async_trait]
#[rust_grpc_lib::async_trait]
impl ExampleDb for ExampleDbService {
    #[roles(any("operator", "admin"))]
    async fn get_end_point_one(&self, ctx: RequestContext<IdList>) -> Result<Response<ReplyOne>, Status> {
        tracing::info!(user = ctx.sub(), ids = ctx.message.id.len(), "getEndPointOne");
        /* ... */
    }

    #[any_authenticated]
    async fn get_end_point_two(&self, ctx: RequestContext<IdList>) -> Result<Response<ReplyTwo>, Status> { /* ... */ }
}
```

## Getting started

1. Add the dependency, which is distributed by git tag rather than crates.io: `rust-grpc-lib = { git = "https://github.com/fermi-ad/rust-grpc-lib", tag = "v8.0.0" }`. Add features as needed (`server` to serve, `build` in `[build-dependencies]`, `test-util` in `[dev-dependencies]`). Besides it, a service needs only `tokio`, `tracing` and `tracing-subscriber` (the library emits tracing data but never installs a subscriber), and a client only `tokio`; no `tonic`, `prost`, `tonic-prost`, `async-trait` or `tonic-health`. Do not rename `rust-grpc-lib`. See [Adding the dependency](docs/server.md#adding-the-dependency).
2. Generate the protos in `build.rs` ([Generating the protos](docs/server.md#generating-the-protos)).
3. Write and serve a service ([Writing an access-controlled service](docs/server.md#writing-an-access-controlled-service), [Building and serving](docs/server.md#building-and-serving)).
4. Call it with a generated client: `authenticated` for gated methods, `public` for `#[public]` ones ([Generated clients](docs/client.md#generated-clients)).

Or start from the [service template](https://github.com/fermi-ad/rust-grpc-service-template), which has all of this set up.

## Documentation

| Guide | Covers |
|---|---|
| [Concepts](docs/concepts.md) | How the pieces fit, and a glossary. Start here if you are new to tonic, JWTs or Keycloak |
| [Server](docs/server.md) | Dependencies, proto generation, policies, contexts, status codes, the builder, environment variables, tracing |
| [Client](docs/client.md) | Generated clients (`public` and `authenticated`), token providers, token forwarding, the channel pool, environment variables |
| [Testing](docs/testing.md) | Handler unit tests, live-server tests, health checks, conformance checks, span capture |
| [Troubleshooting](docs/troubleshooting.md) | Macro compile errors, build and setup problems, runtime surprises |
| [Service template](https://github.com/fermi-ad/rust-grpc-service-template) | A runnable example service built on this crate, to start new services from |
| [API docs](https://fermi-ad.github.io/rust-grpc-lib/rust_grpc_lib/index.html) | The rustdoc, including the feature table. Build it locally with `cargo doc --open` |
| [CHANGELOG](CHANGELOG.md) | Release notes and migration guides |

## Repository layout

| Path | Contents |
|---|---|
| `crates/core` | The `rust-grpc-lib` package |
| `crates/core/interface-definitions` | Git submodule with the bundled `.proto` files ([fermi-ad/interface-definitions](https://github.com/fermi-ad/interface-definitions)) |
| `crates/grpc_macro` | The `grpc-macro` proc macros |
| `crates/integration_tests` | Integration and compile-fail (trybuild) tests |

## Contributing

```bash
git submodule update --init --recursive   # needed in a clone of this repo; consumers do not need it
cargo test --workspace --all-features
TRYBUILD=overwrite cargo test -p integration_tests compile_fail   # regenerate compile-fail expectations, then review the diff
bash scripts/gen-test-fixtures.sh         # regenerate crates/integration_tests/src/fixtures/ after a submodule update
```

- Bump the interface definitions with the **"Bump Interface Definitions"** GitHub workflow rather than by editing the submodule by hand. Anyone who uses this library can run it: it takes `minor` or `major` and a change description, and opens a PR, so it needs little oversight. To do it locally, run `./scripts/update-interface-definitions.sh --minor|--major --description "..."`.
- Releases are tagged `vX.Y.Z`, matching the version in `crates/core/Cargo.toml`.

## License

Apache-2.0. See [`LICENSE`](LICENSE).
