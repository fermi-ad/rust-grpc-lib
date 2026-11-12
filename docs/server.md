# Serving services

See [Concepts](concepts.md) for how the pieces fit. The [service template](https://github.com/fermi-ad/rust-grpc-service-template) is a runnable example service built on this crate.

## Adding the dependency

The crate is distributed by git tag (`vX.Y.Z`) from the fermi-ad GitHub organization, not crates.io. You need Rust 1.85+ (edition 2024), your own Tokio runtime, and git credentials that can read fermi-ad repositories; if private fetches fail, set `[net] git-fetch-with-cli = true` in `.cargo/config.toml`. Cargo fetches the bundled `.proto` files; there is no submodule to initialize.

To start a new service, use the [service template](https://github.com/fermi-ad/rust-grpc-service-template), which has the dependencies already set up; see its `Cargo.toml` for a full example. No feature is on by default; the feature table (what each enables) is in the [crate docs](https://fermi-ad.github.io/rust-grpc-lib/rust_grpc_lib/index.html#features). A typical service lists `rust-grpc-lib` three times:

- `[dependencies]`: `rust-grpc-lib` with `server` (and `jwks-url` if tokens are validated against a JWKS URL), plus `tokio`, `tracing` and `tracing-subscriber`.
- `[build-dependencies]`: `rust-grpc-lib` with `build`, for `build.rs`.
- `[dev-dependencies]`: `rust-grpc-lib` with `test-util`, plus `tokio` (already there if it is in `[dependencies]`), for tests.

That is the whole list. You do **not** add `tonic`, `prost`, `tonic-prost`, `async-trait` or `tonic-health`: the macros and the generated code refer only to `::rust_grpc_lib`, which re-exports what they need. In your own code:

- tonic types come from `rust_grpc_lib::tonic` (`use rust_grpc_lib::tonic::{Request, Response, Status};`).
- prost items come from `rust_grpc_lib::prost` (`rust_grpc_lib::prost::Message`).
- Service impls use `#[rust_grpc_lib::async_trait]`.
- Health is served automatically by `GrpcServer`, so no `tonic-health` is needed; tests query it with `rust_grpc_lib::testing::health_status` / `health_watch`.

Notes:

- A client-only crate drops `server` and `jwks-url`, and needs only `rust-grpc-lib` and `tokio` (see the [client guide](client.md)).
- **Do not rename** `rust-grpc-lib` (no `package = ".."`): macro and generated code refer to `::rust_grpc_lib`.
- **Tracing.** The library emits `tracing` spans and events but never installs a subscriber; the application installs one, usually with `tracing-subscriber` (see [Tracing and logs](#tracing-and-logs)).
- **TLS.** Do not enable tonic's TLS features or configure TLS settings or certificates: the infra team handles TLS, not this library. Services operate over plaintext, and the library enables only tonic's `transport`.
- `test-util` ships a publicly known RSA key: `[dev-dependencies]` only. Nothing stops a release build from enabling it, and `test_validator` in production lets anyone authenticate as any user with any role.

## Generating the protos

```rust
// build.rs
use rust_grpc_lib::build_support::{Config, generate_protos};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    generate_protos(Config::new())
}
```

```rust
// src/lib.rs: include the generated file once, as a whole
pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/proto.rs"));
}
```

- Every bundled proto is compiled with a vendored `protoc`; you cannot select a subset or add your own protos. This is deliberate: all protos come only from the org-wide [interface-definitions](https://github.com/fermi-ad/interface-definitions) repository, so every service and client in the org shares one set of generated types.
- **Need a new or changed RPC or message?** Open a PR in `fermi-ad/interface-definitions`. Once it is merged, run the "Bump Interface Definitions" GitHub workflow of this repository yourself (choose `minor` or `major` and describe the change; see [Contributing](../README.md#contributing)). It opens a PR that picks up the change, deliberately on rails so that it needs little oversight from the code owners. When the release is out, bump the `tag` in your `Cargo.toml`. The [CHANGELOG](../CHANGELOG.md) lists which release picked up which proto change.
- Every bundled proto is compiled into your build, so a version bump can change generated types for protos you do not use. Check the CHANGELOG migration notes when you bump the `tag`.
- **The generated module path.** `package a.b;` becomes `proto::a::b` under the module you `include!` into (`proto` above), and keyword segments are escaped (`r#type`). Inside it, tonic naming applies: a service `ExampleDB` in `package services.example;` yields the client `proto::services::example::example_db_client::ExampleDbClient` and the trait and server `proto::services::example::example_db_server::{ExampleDb, ExampleDbServer}`; messages sit directly in `proto::services::example`. Well-known types are in `proto::google::protobuf`. When unsure, search `$OUT_DIR/proto.rs` for the service name.
- `generate_protos` reads `OUT_DIR` and overwrites `PROTOC`, so call it only from a build script. It fails if a bundled proto is unreadable or has no `package`, if `protoc` or tonic-prost-build fails, if `OUT_DIR` is unset, or if `proto.rs` cannot be written.
- `Config::new()` always derives `GrpcClient` on every client (adding the `public` and `authenticated` constructors; see [Generated clients](client.md#generated-clients)) and reruns the build when a bundled proto changes. Defaults can be added to, not removed.

To add attributes, chain `Config` methods (`type_attribute`, `message_attribute`, `enum_attribute`, `field_attribute`, `client_attribute`, `client_mod_attribute`, `server_attribute`, `server_mod_attribute`). Paths are **protobuf** names, not Rust paths; `"."` matches everything. The `build_support` rustdoc gives the exact path forms and an example per method.

```rust
let config = Config::new()
    .type_attribute(".common.alarm", "#[derive(serde::Serialize, serde::Deserialize)]")
    .field_attribute("common.alarm.Status.device", "#[serde(skip_serializing)]");
```

## Writing an access-controlled service

```rust
use rust_grpc_lib::access_controlled_service;
use rust_grpc_lib::auth::RequestContext;
use rust_grpc_lib::tonic::{Response, Status};

#[access_controlled_service] // must be ABOVE #[rust_grpc_lib::async_trait]
#[rust_grpc_lib::async_trait]
impl ExampleDb for ExampleDbService {
    #[roles(any("operator", "admin"))]
    async fn get_end_point_one(&self, ctx: RequestContext<IdList>) -> Result<Response<ReplyOne>, Status> {
        tracing::info!(user = ctx.sub(), ids = ?ctx.message.id, "lookup");
        /* ... */
    }

    #[any_authenticated]
    async fn get_end_point_two(&self, ctx: RequestContext<IdList>) -> Result<Response<ReplyTwo>, Status> { /* ... */ }
}
```

Every method declares exactly one policy:

| Attribute | Who gets in | Handler parameter |
|---|---|---|
| `#[roles(any("a", "b"))]` | Valid token with at least one listed realm role | `RequestContext<T>` |
| `#[roles(all("a", "b"))]` | Valid token with every listed realm role | `RequestContext<T>` |
| `#[any_authenticated]` | Any valid token | `RequestContext<T>` |
| `#[public]` | Anyone; a token, if sent, is neither read nor validated | `PublicContext<T>` |

- Roles are string literals or paths to `&str` constants.
- `T` is the message, or `rust_grpc_lib::tonic::Streaming<Req>` for client- and bidirectional-streaming RPCs. Bind the context to a plain name (`ctx`, `mut ctx`); a plain `tonic::Request<T>` parameter is a compile error.
- Policy attributes need no import inside the block; `RequestContext` and `PublicContext` must be in scope.
- The macro recognizes policy attributes by their last path segment. Any attribute named `roles`, `any_authenticated` or `public` inside the block, such as an unrelated `#[other_crate::public]`, is treated as a policy and removed. Don't use other crates' attributes with these names on methods in an annotated block.
- Wire names match ignoring case, `_` and `r#` (`get_end_point_one` matches `getEndPointOne`); two methods that collide this way are a compile error.
- If `FooServer` cannot be derived from the trait path, write `#[access_controlled_service(server = path::to::FooServer)]` (no generics).

The full list of compile errors is in the `access_controlled_service` rustdoc; common ones are in [Troubleshooting](troubleshooting.md#macro-compile-errors).

### What the macro rewrites

The macro strips the policy attributes, emits `impl ServicePolicy<FooServer<Type>> for Type` (the table `add_service` requires), and changes each context parameter back to the `tonic::Request<T>` the trait expects, rebuilding your context from it before your body runs. It authenticates nothing. If a context is built for a call that did not pass through the gate, your body never runs and the call fails with `Internal "authentication is not configured"`. Consequences:

- rust-analyzer may show the trait's `Request<T>` signature instead of your `ctx` parameter.
- Unit tests call handlers with a `Request<T>`; see [Testing](testing.md#handler-unit-tests).

## Contexts

| Type | Members |
|---|---|
| `RequestContext<T>` | `claims`, `message`, `sub()`, `exp_unix()`, `is_expired()`, `has_role(..)`, `token()`, `forward(msg)`, `into_forwarded()`, `into_parts()`. `Debug` redacts the token |
| `PublicContext<T>` | `message`. No claims or token, even if the client sent one |
| `CallInfo` (both deref to it) | `rpc` (`RpcMethod { service, method }`, wire names), `remote_addr`, `metadata` (never contains `authorization`), `extensions`, `span()`, `instrument(fut)` |

Events logged in a handler land in the `rpc` span. For spawned tasks, wrap the future with `ctx.instrument(fut)`. Token expiry is checked only when the call starts: long-lived streams should re-check `ctx.is_expired()`. To call other services as the caller, see [Forwarding the caller's token](client.md#forwarding-the-callers-token).

## Status codes

| Situation | Status |
|---|---|
| Service not served by this server (no gate is reached) | `Unimplemented` (from tonic) |
| Path is not `/<service>/<method>`, or the method is not in the policy table | `PermissionDenied "method not permitted"` (logged at `error`) |
| No `authorization` header, or not `Bearer ` (case-sensitive) | `Unauthenticated "missing Authorization header"` |
| Invalid or expired token | `Unauthenticated` (the validator's message) |
| Valid token, roles not satisfied | `PermissionDenied "required role not present"` (logged at `warn`) |
| Context built without the gate | `Internal "authentication is not configured"` |
| Handler still running at the `.timeout()` deadline | `Cancelled` |

The gate removes the `authorization` header from every call, including public calls and rejections. Tokens are never logged.

## Building and serving

```rust
GrpcServer::builder(validator)                 // Arc<impl TokenValidator>
    .timeout(Duration::from_secs(30))          // optional; none by default
    .add_service(ExampleDbServer::new, ExampleDbService)
    .serve_with_shutdown(addr, async { let _ = tokio::signal::ctrl_c().await; })
    .await?;
```

- `add_service` accepts only `#[access_controlled_service]` implementations whose `server` matches the constructor; anything else is a compile error.
- `.timeout(d)` drops the handler at the deadline and the client gets `Cancelled`; a shorter client `grpc-timeout` still wins.
- `serve(addr)` returns `Result<Infallible, ServeError>` (it never returns `Ok`), and `serve_with_shutdown(addr, fut)` and `serve_on_listener(listener, fut)` (your `TcpListener`; bind port 0 in tests) return `Result<(), ServeError>`. `serve_on_listener` has no bind step, so it never returns `ServeError::Bind`. See the `rust_grpc_lib::server` rustdoc.
- `ServeError` and `ChannelError` include the inner error in their `Display` text and also return it from `source()`. Reporters that print the whole chain, such as `anyhow`'s `{:#}` or `{:?}`, therefore show the inner error twice.
- **Health.** `GrpcServer` serves `grpc.health.v1.Health` automatically; you add no `tonic-health` dependency and register nothing. It needs no token, so **any unauthenticated caller can learn which services the server hosts** by checking their names. Don't put anything sensitive in a service name. `""` and every added service report `Serving` at startup and `NotServing` as soon as the shutdown future completes, before draining. Open streams, including health `Watch`, delay shutdown. Tests query it with `testing::health_status` / `health_watch` (see [Testing](testing.md#health-checks)).
- **Inspecting policies.** `PolicyTable::for_service(ExampleDbServer::<ExampleDbService>::new)` (the constructor is never called; it names the types, as in `add_service`) offers `.policies()` and `.lookup("getEndPointOne")`, for example to log routes at startup.

## Environment variables

`GrpcServer` reads none itself. The validator usually comes from `KeyValidatorConfig::from_env()` (from `rust-auth-lib`, re-exported in `rust_grpc_lib::auth`):

| Variable | Meaning |
|---|---|
| `AUTH_JWKS_URL` | JWKS URL. Read only with the `jwks-url` feature; takes precedence over the two below |
| `AUTH_JWKS_FILE` | Path to a JWKS file; takes precedence over `AUTH_PEM_FILE` |
| `AUTH_PEM_FILE` | Path to a PEM public key |
| `AUTH_ISSUER` | Optional: tokens from any other issuer are rejected |
| `AUTH_CACHE_TTL_SECS` | Optional key refresh interval. Default 3600 s for a URL; for a file, unset means the keys are read once |

With no key source set, `from_env` returns a `ConfigError`. Client-side variables (channel pool, `FileTokenProvider`) are in the [client guide](client.md#environment-variables).

## Tracing and logs

The library emits `tracing` spans and events but **never installs a subscriber**, so nothing is printed until your application installs one. Add `tracing-subscriber` and install it once at startup (for example `tracing_subscriber::fmt::init()`); the [service template](https://github.com/fermi-ad/rust-grpc-service-template) shows a full setup.

Every call, including health checks and rejections, gets an INFO span `rpc` on target `rust_grpc_lib::trace`. The canonical field list is in the `rust_grpc_lib::server` rustdoc; what matters for filtering:

| Field | Set when |
|---|---|
| `rpc.system`, `rpc.service`, `rpc.method` | Always (`unknown` service and method for an unparsable path) |
| `grpc.status` | With the response headers for errors, otherwise at end of stream. `Cancelled` if the response future is dropped before it produces a response (client cancel or timeout). A cancel mid-stream leaves it unset, and `stream_ms` absent |
| `latency_ms`, `stream_ms` | Time to response headers; stream duration after them |
| `access` | By the gate: `public`, `authenticated` or `roles`. Absent for health, unparsable paths and unknown methods |
| `user` | Token `sub`, once a token validates (so present on role denials) |

Denials are events on target `rust_grpc_lib::authz`: `warn` "authorization denied" for role denials (fields `user`, `rpc.service`, `rpc.method`, `required_roles`, `role_mode`, `remote_addr`), `error` for unparsable paths and methods missing from the table.

- Span fields appear only if your subscriber logs span close events (for example `FmtSpan::CLOSE`) or you log events inside the span.
- `RUST_LOG=rust_grpc_lib::trace=off` silences per-call spans such as health-probe noise; `RUST_LOG=rust_grpc_lib::authz=warn` shows denials.
