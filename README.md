# rust-grpc-lib

The latest documentation for this crate [can be found here](https://fermi-ad.github.io/rust-grpc-lib/rust_grpc_lib/index.html)

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

---

## Authentication (Zero-Trust JWT)

The `auth` feature is **enabled by default**. All outbound calls carry a `Bearer` token; all inbound calls are validated before reaching your handler.

There are three common service archetypes. Pick the one that matches your service's role in the system.

### Middle-layer service

Sits between the edge and the GraphQL gateway. Validates the incoming user JWT, enforces role-based access control, and forwards the same token downstream.

```rust
use std::sync::Arc;
use rust_grpc_lib::auth::{
    KeyValidator, KeyValidatorConfig,
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
    // Reads AUTH_JWKS_FILE or AUTH_PEM_FILE (or AUTH_JWKS_URL if jwks-url feature is on); optionally AUTH_ISSUER.
    let validator = Arc::new(
        KeyValidator::new(KeyValidatorConfig::from_env()?)?
    );

    tonic::transport::Server::builder()
        .layer(validator_into_layer(validator))
        .add_service(DaqServer::new(MyDaqService))
        .serve("[::1]:50051".parse()?)
        .await?;

    Ok(())
}
```

### Reading claims with `RequestContext`

Instead of fishing the claims out of the request extensions, build a `RequestContext` at the top of a handler. It carries the message, the validated claims, metadata, extensions and the peer address:

```rust
use rust_grpc_lib::auth::RequestContext;

async fn set_data(
    &self,
    request: Request<SetDataRequest>,
) -> Result<Response<SetDataResponse>, Status> {
    let ctx = RequestContext::try_from_request(request)?;
    if !ctx.has_role("operator") {
        return Err(Status::permission_denied("operator role required"));
    }
    tracing::info!(user = ctx.sub(), peer = ?ctx.remote_addr, "set_data");
    let SetDataRequest { .. } = ctx.message;
    todo!()
}
```

`RequestContext<T, C = KeycloakClaims>` is generic over the claims type; `C` must implement `Claims` (and be `Clone` to call `try_from_request`). `has_role()` is only available for `KeycloakClaims`. `try_from_request` also works for streaming requests (`RequestContext<Streaming<Req>>`). `is_expired()` and `exp_unix()` let a long-lived stream decide when to end. If the JWT layer is not installed, it returns `Status::internal("authentication is not configured")` and logs the detail server-side only.

Inside a `#[keycloak_authenticated_service]` impl you do not need to call `try_from_request` yourself: declare the parameter as `RequestContext<T>` and the macro does it for you (see below).

### Keeping health checks open (per-service auth)

`Server::layer(validator_into_layer(..))` wraps every service, so unauthenticated health probes are rejected too. There is no exemption list. To keep a service open, authenticate each application service individually and add the open service unwrapped:

```rust
use rust_grpc_lib::auth::{KeycloakClaims, wrap_service};

let (mut reporter, health) = tonic_health::server::health_reporter();
reporter.set_serving::<DaqServer<MyDaqService>>().await;

tonic::transport::Server::builder()
    .add_service(health) // no JWT required
    .add_service(wrap_service::<KeycloakClaims, _, _>(
        DaqServer::new(MyDaqService),
        validator,
    ))
    .serve("[::1]:50051".parse()?)
    .await?;
```

`wrap_service` needs no direct `tower` dependency and keeps `NamedService`, so `set_serving` keeps working. Equivalent: `DaqServer::with_interceptor(MyDaqService, JwtValidationService::<KeycloakClaims, _>::new(validator))`.

### Tracing every RPC (`trace` feature)

The opt-in `trace` feature adds `rust_grpc_lib::trace::GrpcTraceLayer`, which opens one `rpc` span per request and keeps it open until the response stream ends, so unary and streaming RPCs are both covered. Install it with `Server::layer` **before any other layer** so that it is outermost and also sees requests that authentication rejects:

```rust
use rust_grpc_lib::trace::GrpcTraceLayer;

tonic::transport::Server::builder()
    .layer(GrpcTraceLayer::new())
    .add_service(health)
    .add_service(wrap_service::<KeycloakClaims, _, _>(DaqServer::new(MyDaqService), validator))
    .serve("[::1]:50051".parse()?)
    .await?;
```

Each span has the fields `rpc.system`, `rpc.service`, `rpc.method`, `grpc.status`, `latency_ms`, `stream_ms` and `user`, and the target `rust_grpc_lib::trace`. The layer also inserts the `RpcMethod` and `RpcSpan` request extensions, which is what fills `RequestContext::rpc_method` and the `user` field. See the [`trace`](crates/core/src/trace.rs) module docs for what each field means, how cancellation and timeouts are recorded, and why `user` is recorded onto the span directly.

`Server::layer` wraps every service, so health probes get a span too; filter the `rust_grpc_lib::trace` target if they are noise. The layer does not authenticate anything. It adds `tower` and `tower-http` to the build, which is why it is behind a feature.

### Testing your service (`test-util`)

Enable the `test-util` feature under `[dev-dependencies]` to get `rust_grpc_lib::testing`. It re-exports `rust-auth-lib`'s `KeycloakClaimsBuilder` and `test_fixtures`, so you do not need a direct dependency on it.

```toml
[dev-dependencies]
rust-grpc-lib = { git = "...", tag = "vX.Y.Z", features = ["test-util"] }
```

Unit-test a handler without a server or a JWT:

```rust
use rust_grpc_lib::auth::RequestContext;
use rust_grpc_lib::testing::{claims_with_roles, request_with_claims};

let claims = claims_with_roles("alice", &["operator"]);

// A handler that takes `RequestContext<T>`:
let ctx = RequestContext::for_test(claims.clone(), SetDataRequest::default());

// A handler that still takes `Request<T>` (including `#[roles]`-guarded ones):
let request = request_with_claims(SetDataRequest::default(), claims);
```

`claims_with_roles` covers the common case. Use `KeycloakClaimsBuilder` directly when a test also needs a custom expiry or other claims.

Assert on `tracing` spans (for example what `GrpcTraceLayer` or `RequestContext` recorded) with `capture_spans`. It installs a thread-local subscriber, so use the default current-thread `#[tokio::test]` runtime:

```rust
use rust_grpc_lib::testing::capture_spans;

let (spans, _guard) = capture_spans();
// ... call the code under test ...
assert_eq!(spans.field("rpc", "grpc.status").as_deref(), Some("Ok"));
```

Check the wiring of a running server with the conformance helpers. The library cannot enumerate your RPCs, so pass an explicit list of `"/package.Service/Method"` paths. Build the service part from the `SERVICE_NAME` const that tonic generates next to every server (it is the fully qualified `package.Service` name, and the same value the server reports through `NamedService::NAME`), so a renamed package or service is a compile error in your test instead of a silent `unimplemented`:

```rust
use rust_grpc_lib::testing::{assert_all_rpcs_require_auth, assert_all_rpcs_require_role};
use my_proto::daq_server::SERVICE_NAME; // generated: "pkg.Daq"

let channel = tonic::transport::Channel::from_shared(format!("http://{addr}"))?
    .connect()
    .await?;
let read = format!("/{SERVICE_NAME}/ReadData");
let set = format!("/{SERVICE_NAME}/SetData");

// No token and a malformed token are both `Unauthenticated`.
assert_all_rpcs_require_auth(channel.clone(), &[&read, &set]).await;
// A valid token without the role is `PermissionDenied` (only list RPCs that declare roles).
assert_all_rpcs_require_role(channel, &[&set], &viewer_token).await;
```

`SERVICE_NAME` also works for the health reporter and for filtering health out of a list of paths. The `Method` part still has to be typed by hand, exactly as the proto spells it and as it appears in the generated server's `match req.uri().path()` (the `ExampleDB` fixture, for example, defines `getEndPointOne`, not `GetEndPointOne`). A wrong method name is answered with `unimplemented`: the role check reports that as a failure, but the no-token check cannot, because auth rejects the call before routing.

Do not list the health service: it is unauthenticated by design, and the helpers panic if you do. Do not enable `test-util` in a production build.

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

Applied to an `impl Trait for Type` block. Injects Keycloak role-checking guards into methods annotated with `#[roles(...)]`. A valid JWT is still required by the JWT validation layer for every method; methods without `#[roles(...)]` just get no role check from this macro.

**Placement.** Write it *above* `#[async_trait]` / `#[tonic::async_trait]`. The wrong order is a compile error that says so.

```rust
#[rust_grpc_lib::keycloak_authenticated_service]
#[tonic::async_trait]
impl Daq for MyDaqService { /* ... */ }
```

**`RequestContext` parameters.** The first non-`self` parameter of a method may be a `RequestContext<T>` instead of a `tonic::Request<T>`. The macro rewrites the emitted signature back to `Request<T>` (so it still matches the generated trait) and starts the body with `RequestContext::try_from_request(..)?`. The role guard reuses `ctx.claims`, so the claims are looked up once. Both forms can be mixed in one impl, and streaming requests work (`RequestContext<Streaming<Req>>`).

```rust
#[roles(all("operator"))]
async fn set_data(
    &self,
    ctx: RequestContext<SetDataRequest>,
) -> Result<Response<SetDataResponse>, Status> {
    tracing::info!(user = ctx.sub(), "set_data");
    // ctx.message, ctx.claims, ctx.metadata, ctx.remote_addr ...
    todo!()
}
```

**Custom claims.** `#[keycloak_authenticated_service(claims = MyClaims)]` selects the claims type that `Request<T>` methods read from the extensions (default `KeycloakClaims`). `MyClaims` must implement `Claims` and have a `has_role(&self, &str) -> bool` method. A `RequestContext<T, C>` parameter uses its own `C`.

**Consumer dependencies.** The expansion names `::rust_grpc_lib`, `::tonic` and `::tracing`, so it uses whatever versions your crate depends on; this library neither pins nor re-exports them. Three things follow, and they cannot be avoided:

- Depend on `rust-grpc-lib`, `tonic` and `tracing` under their normal names. Renaming a dependency (`package = ".."`) is not supported.
- Your crate needs `tonic` and `tracing` as direct dependencies, because the guard's denial log is emitted through `::tracing`. Logging is not optional and there is no feature to turn it off.
- Your `tonic` must be semver-compatible with the one `rust-grpc-lib` is built against, because `RequestContext` and the guard exchange `tonic::Request` and `tonic::Status` with your handlers.

**Denied calls are logged.** When a caller lacks the required roles the client receives `permission_denied("required role not present")`, and the server emits a `warn` event with target `rust_grpc_lib::authz` and message `authorization denied`. Its fields are `user` (the `sub` claim), `rpc.service` (the trait name), `rpc.method` (the Rust method name), `required_roles`, `role_mode` (`any` or `all`) and `remote_addr` when known. The token, the other claims and the roles the caller actually holds are never logged. Filter or route it with `RUST_LOG=rust_grpc_lib::authz=warn`. Repeated denials are not rate limited by the library; use your subscriber's filters if that matters.

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
| `jwks-url` | off | Enables JWKS key retrieval and rotation from a URL via `KeyValidatorConfig::from_jwks_url` (opt-in; pulls in an HTTP client) |
| `unauthenticated` | off | Enables `from_endpoint` (no-auth constructor) on generated clients; for test harnesses only |
| `build` | off | Enables proto code-generation helpers (`build_support` module) |
| `test-util` | off | Enables the `testing` module: claims builders, `RequestContext::for_test`, claim injection, span capture and auth conformance helpers; for `[dev-dependencies]` only |
| `trace` | off | Enables `trace::GrpcTraceLayer` (a span per RPC) and the `RpcMethod`/`RpcSpan` request extensions; adds `tower` and `tower-http` (implies `auth`) |

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

### Integrating new versions of `interface-definitions`

If you are simply here to integrate the latest Protobuf definitions from `interface-definitions`, there's a handy script you can run.

First, make sure you know what changes you're expecting to bring in from `interface-definitions`. Evaluate whether any existing consumers of this crate will be exposed to breaking changes after the integration is complete. If so, this is a major version change. If not, it is a minor version change.

```bash
# From the project root
./scripts/update-interface-definitions.sh --minor --description "Describe the new interfaces" # pass --major instead if making a breaking change
```

That script will pull the latest version of `interface-definitions`, update the relevant `Cargo.toml` with the next version, and add the description to the changelog.

**New** There is also a GitHub Workflow in this repository to run this operation for you. Simply go to the Actions tab and initiate the "Bump Interface Definitions" workflow. Select the version change and provide a description of the new interfaces. It will run the script and commit the changes to a new branch. A PR will automatically be opened for you to update and add reviewers.

Don't forget to make sure a matching tag is added to the repository once your changes are merged to `main`! (Auto-tagging coming soon)

### Repository layout

This is a Cargo workspace with three crates:

| Crate | Path | Description |
|---|---|---|
| `rust-grpc-lib` | [`crates/core/`](crates/core/) | The main library crate consumers depend on. Contains the connection pool, auth wiring, and build-support helpers. |
| `grpc-macro` | [`crates/grpc_macro/`](crates/grpc_macro/) | Proc-macro crate providing `#[derive(GrpcClient)]`, `#[derive(GrpcNoAuthClient)]`, `#[keycloak_authenticated_service]`, and `#[roles(...)]`. Re-exported from the main crate — consumers never need to depend on this directly. |
| `integration_tests` | [`crates/integration_tests/`](crates/integration_tests/) | Integration-test crate. Exercises the full client→server gRPC round-trip with real JWT auth, tests the `#[keycloak_authenticated_service]` macro expansion, and holds the `trybuild` compile-fail cases in `src/compile_fail_cases/`. |

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
