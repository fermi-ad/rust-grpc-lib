# Serving services

How to build, configure and run a gRPC server with `GrpcServer`: dependencies, proto generation, access policies, serving, shutdown, token validation, metrics and logs. Concepts first? See [Concepts](concepts.md). Something broken? See [Troubleshooting](troubleshooting.md).

## Adding the dependency

The `Cargo.toml` entries are in the [README](../README.md#install). Rules that are easy to miss:

- **Features.** None is on by default.

  | Feature | Enables |
  |---|---|
  | `server` | `GrpcServer`, `#[access_controlled_service]` and the policy attributes, `RequestContext` / `PublicContext`, the `telemetry` module |
  | `jwks-url` | `AUTH_JWKS_URL` and `KeyValidatorConfig::from_jwks_url` (fetch keys over HTTP) |
  | `build` | `build_support::generate_protos`; goes in `[build-dependencies]` |
  | `test-util` | The `testing` module (implies `server`); goes in `[dev-dependencies]` |

  A client-only crate needs no feature.
- **Do not add** `tonic`, `prost`, `tonic-prost`, `async-trait` or `tonic-health`, and **do not rename** the dependency (no `package = ".."`). Use `rust_grpc_lib::tonic`, `rust_grpc_lib::prost` and `#[rust_grpc_lib::async_trait]`; the macros and generated code refer only to `::rust_grpc_lib`.
- **Git access.** The crate comes from the fermi-ad GitHub organization by tag. If private fetches fail, set `[net] git-fetch-with-cli = true` in `.cargo/config.toml`. The bundled `.proto` files come with the crate; there is no submodule to initialize.
- **TLS.** Do not enable tonic's TLS features or configure certificates. Services speak plaintext and the infra team terminates TLS (see [Concepts](concepts.md#security-assumption)).
- **Logging.** The library emits `tracing` events but installs no subscriber (see [Tracing and logs](#tracing-and-logs)).

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

- **Every bundled proto is compiled** with a vendored `protoc` (a `PROTOC` you set is overwritten). You cannot select a subset or add your own protos: all of them come from the org-wide [interface-definitions](https://github.com/fermi-ad/interface-definitions) repository so every service and client shares one set of types. A version bump can therefore change generated types you do not use; read the [CHANGELOG](../CHANGELOG.md) migration notes when you bump the `tag`.
- **Need a new or changed RPC or message?** Open a PR in `fermi-ad/interface-definitions`. After it merges, run this repository's "Bump Interface Definitions" GitHub workflow (choose `minor` or `major` and describe the change; see [Contributing](../README.md#contributing)), then bump the `tag` in your `Cargo.toml` once the release is out.
- **Module paths.** `package a.b;` becomes `proto::a::b` (Rust keywords are escaped, as in `r#type`). Tonic naming applies inside it: service `ExampleDB` in `package services.example;` gives `proto::services::example::example_db_client::ExampleDbClient` and `proto::services::example::example_db_server::{ExampleDb, ExampleDbServer, SERVICE_NAME}`. Messages sit directly in `proto::services::example`; well-known types are in `proto::google::protobuf`. When unsure, search `$OUT_DIR/proto.rs` for the service name.
- **Call it only from a build script.** `generate_protos` reads `OUT_DIR`, overwrites `PROTOC`, and fails if a proto is unreadable or has no `package`, if `protoc` fails, or if `proto.rs` cannot be written.
- **Defaults you cannot remove:** every client derives `GrpcClient` (adding `public` / `authenticated`, see [Call a service](client.md#call-a-service)) and the build reruns when a bundled proto changes.

To add attributes, chain `Config` methods: `type_attribute`, `message_attribute`, `enum_attribute`, `field_attribute`, `client_attribute`, `client_mod_attribute`, `server_attribute`, `server_mod_attribute`. Paths are **protobuf** names, not Rust paths; `"."` matches everything. The `build_support` rustdoc gives the exact path form for each method.

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
        Ok(Response::new(ReplyOne::default()))
    }

    #[any_authenticated]
    async fn get_end_point_two(&self, ctx: RequestContext<IdList>) -> Result<Response<ReplyTwo>, Status> {
        Ok(Response::new(ReplyTwo::default()))
    }
}
```

Every method declares exactly one policy:

| Attribute | Who gets in | Handler parameter |
|---|---|---|
| `#[roles(any("a", "b"))]` | Valid token with at least one listed realm role | `RequestContext<T>` |
| `#[roles(all("a", "b"))]` | Valid token with every listed realm role | `RequestContext<T>` |
| `#[any_authenticated]` | Any valid token | `RequestContext<T>` |
| `#[public]` | Anyone; a token, if sent, is neither read nor validated | `PublicContext<T>` |

- Roles are string literals or paths to `&str` constants. Only **realm** roles count; client roles never match.
- `T` is the message, or `rust_grpc_lib::tonic::Streaming<Req>` for client- and bidirectional-streaming RPCs. Bind the context to a plain name (`ctx`, `mut ctx`); a plain `tonic::Request<T>` parameter is a compile error.
- Policy attributes need no import inside the block; `RequestContext` and `PublicContext` must be in scope.
- Wire names match the Rust names ignoring case, `_` and `r#` (`get_end_point_one` matches `getEndPointOne`). Two methods that collide this way are a compile error.
- If `FooServer` cannot be derived from the trait path, write `#[access_controlled_service(server = path::to::FooServer)]` (no generics).
- The macro only declares policies; the gate that `add_service` installs enforces them. A context built for a call that skipped the gate makes the call fail with `Internal "authentication is not configured"` before your body runs. Unit tests therefore call handlers with a `Request<T>` (see [Unit-test a handler](testing.md#unit-test-a-handler)), and rust-analyzer may show the trait's `Request<T>` signature instead of `ctx`.

Compile errors are listed in the `access_controlled_service` rustdoc; the common ones are in [Troubleshooting](troubleshooting.md#macro-compile-errors).

### Contexts

| Type | Members |
|---|---|
| `RequestContext<T>` | `claims`, `message`, `sub()`, `exp_unix()`, `is_expired()`, `has_role(..)`, `token()`, `forward(msg)`, `into_forwarded()`, `into_parts()`. `Debug` redacts the token |
| `PublicContext<T>` | `message`. No claims or token, even if the client sent one |
| `CallInfo` (both deref to it, read-only) | `rpc` (`RpcMethod { service, method }`, proto wire names), `remote_addr`, `metadata` (never contains `authorization`), `extensions`, `span()`, `instrument(fut)` |

Events logged in a handler land in the `rpc` span. For spawned tasks, wrap the future with `ctx.instrument(fut)`. Token expiry is checked only when the call starts, so long-lived streams should re-check `ctx.is_expired()`. To call other services as the caller, see [Forward the caller's token](client.md#forward-the-callers-token).

### Status codes

| Situation | Status |
|---|---|
| Path is not `/<service>/<method>`, or the method is not in the policy table | `PermissionDenied "method not permitted"` (logged at `error`) |
| No `authorization` header, or not `Bearer ` (case-sensitive) | `Unauthenticated "missing Authorization header"` |
| Invalid or expired token | `Unauthenticated` (the validator's message, for example `JWT validation failed: ..`) |
| Valid token, roles not satisfied | `PermissionDenied "required role not present"` (logged at `warn`) |
| Context built without the gate | `Internal "authentication is not configured"` |
| Handler still running at the `.timeout()` deadline | `Cancelled` |

The gate removes the `authorization` header from every call, public calls and rejections included, and tokens are never logged.

## Building and serving

```rust
GrpcServer::builder(validator)                 // Arc<impl TokenValidator>
    .timeout(Duration::from_secs(30))          // optional; none by default
    .add_service(ExampleDbServer::new, ExampleDbService)
    .serve("0.0.0.0:50051".parse()?)
    .await?;
```

- `add_service` accepts only `#[access_controlled_service]` implementations whose server type matches the constructor; anything else is a compile error. Use the generated server type from `generate_protos`: it carries the wire-method metadata `add_service` needs.
- `.timeout(d)` drops the handler at the deadline and the client gets `Cancelled`; a shorter client `grpc-timeout` still wins.
- **Health.** `grpc.health.v1.Health` is served automatically and needs no token. `""` and every added service report `Serving` at startup and `NotServing` the moment shutdown begins, before draining.
- **Inspecting policies.** `PolicyTable::for_service(ExampleDbServer::<ExampleDbService>::new)` gives `.policies()` and `.lookup("getEndPointOne")`, for example to log routes at startup. The constructor is never called; it only names the types.
- **Errors.** `serve*` returns `ServeError`, which is `#[non_exhaustive]`:

  | Variant | Display starts with | From |
  |---|---|---|
  | `Bind` | `failed to bind the listen address:` | `serve`, `serve_with_shutdown` |
  | `Signal` | `failed to register the shutdown signal handlers:` | `serve` only |
  | `Transport` | `gRPC transport error:` | all |

### Graceful shutdown

| Method | Registers signal handlers | Shutdown trigger | Can return |
|---|---|---|---|
| `serve(addr)` | Yes | SIGTERM or SIGINT (Unix), Ctrl-C (Windows) | `Bind`, `Signal`, `Transport` |
| `serve_with_shutdown(addr, future)` | No | Your future | `Bind`, `Transport` |
| `serve_on_listener(listener, future)` | No | Your future (pre-bound listener, e.g. port `0` in tests) | `Transport` |

Run all of them inside a Tokio runtime with I/O and time enabled (`enable_all()` suffices). `serve` registers handlers before binding; on Unix that replaces the default behavior of those signals for the rest of the process. `.without_signal_handling()` makes `serve` run until it fails or its future is dropped. To get the same signals in your own trigger, `await` `server::shutdown_signal()` (returns `io::Result<()>`).

Once the trigger fires:

1. Health reports `NotServing` for `""` and every service, so load balancers stop sending traffic. The runtime probe stops.
2. The server stops accepting connections; in-flight RPCs, including health `Watch` streams, may finish for up to `.shutdown_grace(duration)` (**30 seconds** by default, independent of `.timeout()`).
3. At expiry it logs `warn` "shutdown grace expired; closing remaining connections" and closes what is left, open streams included. That is not an error: `serve*` returns `Ok(())` unless the transport failed. A grace of zero skips the drain; there is no unbounded setting.
4. Server-owned telemetry is then flushed, for up to five more seconds (see [Provider ownership](#provider-ownership)).

Set a process termination deadline (for example Kubernetes' `terminationGracePeriodSeconds`) above grace plus flush: 35 seconds with the defaults. Dropping the serve future closes connections but is no substitute for letting it finish.

## Environment variables

`KeyValidatorConfig::from_env()` (from `rust-auth-lib`, re-exported in `rust_grpc_lib::auth`) builds the validator from these:

```rust
let validator = Arc::new(KeyValidator::new(KeyValidatorConfig::from_env()?)?);
```

| Variable | Meaning |
|---|---|
| `AUTH_JWKS_URL` | JWKS URL. Read only with the `jwks-url` feature; takes precedence over the two below |
| `AUTH_JWKS_FILE` | Path to a JWKS file; takes precedence over `AUTH_PEM_FILE` |
| `AUTH_PEM_FILE` | Path to a PEM public key |
| `AUTH_ISSUER` | Optional. Tokens from any other issuer are rejected |
| `AUTH_CACHE_TTL_SECS` | Optional key refresh interval. Default 3600 for a URL; for a file, unset means the keys are read once at startup |

With no key source set, `from_env` returns `ConfigError` ("environment variable AUTH_JWKS_FILE or AUTH_PEM_FILE not set"); `KeyValidator::new` fails if the keys cannot be loaded. Without code, `KeyValidatorConfig::from_jwks_file`, `from_pem_file`, `from_jwks_str`, `from_jwks_url` (`jwks-url`) and `.with_issuer(..)` build the same thing. Client-side variables (channel pool, `FileTokenProvider`) are in the [client guide](client.md#environment-variables); metrics variables are in [Export metrics](#export-metrics).

## Telemetry

With the `server` feature, every server records call metrics for application RPCs, health checks, gate rejections and unknown paths. The metrics layer wraps tracing and access control. Export is off until you configure an endpoint.

### Export metrics

Set the collector's metrics URL and a service name before launching:

```sh
export OTEL_EXPORTER_OTLP_METRICS_ENDPOINT=http://localhost:4318/v1/metrics
export OTEL_SERVICE_NAME=example
```

No code is needed: the builder reads the environment on the first `meter()` call or when serving starts. The endpoint must be an absolute `http` or `https` URL for **OTLP/HTTP with protobuf**, not OTLP/gRPC. Startup does not check that the collector is reachable.

| Variable | Default and behavior |
|---|---|
| `OTEL_EXPORTER_OTLP_METRICS_ENDPOINT` | Full metrics URL, used as is. No default; empty counts as unset |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | Base URL; `/v1/metrics` is appended when the variable above is unset. Empty counts as unset |
| `OTEL_SDK_DISABLED` | `true` (case-insensitive) disables export even with an endpoint |
| `OTEL_METRIC_EXPORT_INTERVAL` | Milliseconds, positive integer. Unset, zero or invalid: 10000 |
| `OTEL_SERVICE_NAME` | Non-empty value wins over `service.name` in `OTEL_RESOURCE_ATTRIBUTES`. Fallback: `unknown_service:<executable name>` |
| `OTEL_RESOURCE_ATTRIBUTES` | Standard `k=v,k=v` resource attributes. `service.version` is never inferred from the crate; set it here or with `.service_version(..)` |
| `GRPC_DURATION_BUCKETS` | Comma-separated seconds, for example `0.005,0.015,0.05,0.2`; must be non-empty, finite, positive and strictly increasing, else a warning and the defaults |
| `GRPC_RUNTIME_PROBE` | `true` (default) or `false`, case-insensitive; anything else warns and uses the default |

If there is no endpoint, the SDK is disabled, or exporter setup fails (logged at `error` on target `rust_grpc_lib::telemetry`, "failed to start telemetry; using a no-op meter"), the meter is a no-op: nothing is exported and the runtime probe does not run, but an `.on_call_complete` observer still runs. `.without_telemetry()` removes the metrics layer and observer entirely; tracing, health and access control are unchanged.

For programmatic settings, start from `TelemetryConfig::from_env()`, apply setters and pass the result to the builder (`TelemetryConfig::default()` reads no environment variables):

```rust
let config = TelemetryConfig::from_env()
    .endpoint("http://localhost:4318/v1/metrics") // full URL; empty clears it
    .service_name("example")                      // empty restores the executable-based fallback
    .service_version("1.2.3")
    .duration_bucket_bounds_seconds(vec![0.005, 0.05, 0.5])
    .runtime_scheduling_probe(false);
let builder = GrpcServer::builder(validator).telemetry_config(config);
```

Also `.sdk_disabled(bool)` and `.export_interval(Duration)`. If you call `.telemetry_config(..)`, `.with_caller_owned_telemetry(..)` or `.without_telemetry()` more than once, the last wins. Choose before calling `meter()`: instruments already created stay on their original meter.

### Built-in metrics

| Instrument | Unit | Measures | Attributes |
|---|---|---|---|
| `rpc.server.call.duration` | `s` (histogram) | Receipt to final status or drop; the count is completed calls | `rpc.system.name` = `grpc`, `rpc.method`, `rpc.response.status_code` (numeric, `i64`), `error.type` only when not OK (tonic code name such as `PermissionDenied`) |
| `rpc.server.active_requests` | `{request}` (up-down counter) | +1 on receipt, −1 on completion or cancellation | `rpc.system.name`, `rpc.method` |
| `runtime.scheduling_delay` | `s` (histogram) | How late Tokio wakes a 10 ms timer; missed ticks are skipped, not replayed | none |

- **Completion** is read from the `grpc-status` header or trailer. Dropping an unfinished response or body (server timeout, client cancels a stream) records `Cancelled`. A body that ends without a status, or an inner error, records `Unknown`.
- **`rpc.method`** is the proto wire name without a leading slash, for example `services.example.ExampleDB/getEndPointOne`; health uses `grpc.health.v1.Health/Check` and `/Watch`. Anything else, including wrong casing and unknown paths, is `_OTHER`; callers cannot create labels. A policy that does not map to exactly one wire method logs a `warn` ("policy does not identify exactly one wire method; omitting telemetry label", with `reason` `missing` or `ambiguous`) and that method reports as `_OTHER`. (On the `rpc` trace span, `rpc.method` is the method segment alone and `rpc.service` is separate.)
- **Status and error naming** deliberately differ from OpenTelemetry v1.37's string conventions: numeric status, tonic code names.
- **Duration buckets** default to `telemetry::DEFAULT_DURATION_BUCKET_BOUNDS_SECONDS`: 36 bounds at 1, 1.5, 2, 3, 5 and 7 per decade from 10 µs to 7 s. Override with `GRPC_DURATION_BUCKETS` or `TelemetryConfig::duration_bucket_bounds_seconds`.
- **Runtime probe.** When enabled, servers sharing a provider share one probe on the first server's runtime. It stops when the last server starts draining or stops. On a multithreaded runtime it detects broad starvation, not one blocked worker.

### Application metrics and call observation

Create instruments with `builder.meter()` (before serving) or `Telemetry::meter()`; the API is re-exported as `rust_grpc_lib::telemetry::opentelemetry`. `Telemetry::duration_histogram(name, description)` applies the configured buckets and seconds unit. Record durations in seconds, choose attribute values from closed sets, use your own instrument namespace, and leave the service name out of instrument names (`service.name` already carries it).

`.on_call_complete(|summary| ..)` observes every completion, cancellations included, after the built-in measurements. `CallSummary` has `method` (`Option<&'static str>`, `None` for `_OTHER`), `status` (`tonic::Code`) and `duration`. It runs on the serving path: keep it cheap and non-blocking. A panic is caught and logged ("gRPC call observer panicked") without changing the RPC result.

### Provider ownership

The library never installs a global meter provider.

| Mode | How | Who flushes |
|---|---|---|
| Server-owned (default, or `.telemetry_config(config)`) | The builder starts the provider | `serve*` flushes and shuts it down after drain, forced close or transport failure. If binding or signal registration fails first, there is no awaited flush for a provider already started by `meter()` |
| Caller-owned | `Telemetry::start(config)` then `.with_caller_owned_telemetry(&telemetry)` | You: `telemetry.shutdown().await` on a Tokio runtime with time enabled. Only the first call across clones waits. Dropping the last handle cleans up but does not wait for a flush |

Awaited cleanup runs off the Tokio runtime and waits up to five seconds. Failures and timeouts are logged at `warn` on `rust_grpc_lib::telemetry` ("telemetry flush or shutdown failed", "telemetry shutdown timed out after five seconds"), never returned as `ServeError`.

If you own the provider, keep the serve result so you flush even when serving fails:

```rust
use std::{net::SocketAddr, sync::Arc};
use rust_grpc_lib::{
    auth::TokenValidator,
    server::{GrpcServer, ServeError},
    telemetry::{Telemetry, TelemetryConfig},
};

async fn serve<V: TokenValidator>(validator: Arc<V>, addr: SocketAddr) -> Result<(), ServeError> {
    let telemetry = Telemetry::start(TelemetryConfig::from_env().service_name("example"));
    let builder = GrpcServer::builder(validator).with_caller_owned_telemetry(&telemetry);
    let calls = builder.meter().u64_counter("app.rpc.finished").build();
    let result = builder.on_call_complete(move |_| calls.add(1, &[])).serve(addr).await;
    telemetry.shutdown().await;
    result
}
```

## Tracing and logs

Nothing is printed until your application installs a subscriber, for example `tracing_subscriber::fmt::init()` at startup (add `tracing-subscriber` to `[dependencies]`).

Every call, health checks and rejections included, gets an INFO span `rpc` on target `rust_grpc_lib::trace`:

| Field | Set when |
|---|---|
| `rpc.system`, `rpc.service`, `rpc.method` | Always (`unknown` service and method for an unparsable path). Here `rpc.method` is the proto method name, without the service |
| `grpc.status` | With the response headers for errors, otherwise at end of stream. `Cancelled` if the response future is dropped before a response (client cancel, timeout); unset after a mid-stream cancel |
| `latency_ms`, `stream_ms` | Time to response headers; stream duration after them (`stream_ms` is absent after a mid-stream cancel) |
| `access` | `public`, `authenticated` or `roles`, set by the gate. Absent for health, unparsable paths and unknown methods |
| `user` | Token `sub`, once a token validates (so present on role denials) |

Other log events:

| Target and level | Message | Meaning |
|---|---|---|
| `rust_grpc_lib::authz` `warn` | "authorization denied" (`user`, `rpc.service`, `rpc.method`, `required_roles`, `role_mode`, `remote_addr`) | Role denial |
| `rust_grpc_lib::authz` `error` | "request path is not of the form /<service>/<method>; denying" / "method has no access policy in the service's policy table; denying" | Unparsable path or method missing from the table |
| `rust_grpc_lib::trace` `warn` | "rpc failed" (`error`, `latency_ms`) | Transport or body failure, inside the `rpc` span |
| `rust_grpc_lib::server::shutdown` `info` | "shutdown signal received" (`signal`) | `serve` got SIGTERM, SIGINT or Ctrl-C |

- Span fields show only if your subscriber logs span close events (for example `FmtSpan::CLOSE`) or you log events inside the span.
- `RUST_LOG=rust_grpc_lib::trace=off` silences per-call spans (health-probe noise); `RUST_LOG=rust_grpc_lib::authz=warn` shows denials only.
