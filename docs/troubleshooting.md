# Troubleshooting

Symptom, cause, fix, most common first. Quoted messages are the ones the library actually produces. For how the pieces fit, see [Concepts](concepts.md).

## A call fails with a status

| Symptom | Cause | Fix |
|---|---|---|
| `Unauthenticated "missing Authorization header"` | No token, or the header is not `Bearer <token>` (case-sensitive). Typical: a `public` client calling a gated method, or a forwarded token that was not a valid header value | Build the client with `authenticated(url, provider)`; in a handler use `ctx.token()` or `ctx.into_forwarded()`. See [Call a service](client.md#call-a-service) |
| `Unauthenticated "JWT validation failed: .."` (for example `ExpiredSignature`, `InvalidSignature`, `InvalidIssuer`) | The token is expired, tampered with, or from an issuer other than `AUTH_ISSUER` | Fetch a fresh token from the right issuer. A client's `FileTokenProvider` re-reads its file after `SERVICE_TOKEN_CACHE_TTL_SECS` (default 30), so check the file itself is fresh |
| `Unauthenticated "signing key not found in JWKS (kid: ..)"` | The server's keys do not include the token's `kid`: wrong realm or environment, or keys rotated since the server loaded a file | Point the client and server at the same Keycloak realm. For file keys set `AUTH_CACHE_TTL_SECS` so they refresh; otherwise restart the server |
| `Unauthenticated "key cache has not been populated or is expired"`, or `"failed to fetch from remote: .."` | The server could not fetch or refresh keys from `AUTH_JWKS_URL` | Check the URL is reachable from the server and serves a JWKS |
| `Unauthenticated` from a client, with the provider's message | The client's `TokenProvider` failed (for example `failed to read token file: ..` from `FileTokenProvider`) | Fix the provider's source (file path, permissions). The call never reached the server |
| `PermissionDenied "required role not present"` with a valid token | The token lacks the required **realm** role. Client roles (`resource_access`) never count; for `all(..)` every role is needed | Grant the realm role in Keycloak or change the policy. The server logs a `warn` "authorization denied" on target `rust_grpc_lib::authz` with `required_roles` and `role_mode` (never the roles held) |
| `PermissionDenied "method not permitted"` | The path is not `/<service>/<method>`, or the method is not in the service's policy table. The server logs an `error` on `rust_grpc_lib::authz` naming the service and method | Check the client calls a method of a service you registered with `add_service`; if you use `server = ..` on the macro, check it names the generated server |
| `Cancelled` | The server's `.timeout(d)` fired and dropped the handler, the client's `grpc-timeout` was shorter, or the client dropped the call | Raise the timeout or speed up the handler |
| `Internal "authentication is not configured"` | A context was built for a call that did not pass the gate: the service was not served through `GrpcServer::add_service`, or a unit test called the handler with `Request::new` or the wrong helper | Serve through `GrpcServer`. In unit tests use `authenticated_request` (gated methods) or `public_request` (`#[public]`). The detail is in the server's `error` log |
| `Internal "token contains invalid header characters: .."` | The client's provider returned a token that is not a valid header value | Fix the token source |
| `Unimplemented` | The method does not exist on that service, or you called a Rust-cased name on the wire (`assert_policy_table_conforms` does this by design for public methods) | Use the proto spelling (`getEndPointOne`); check client and server were generated from the same interface-definitions version |
| `Unavailable` on the first RPC though the client was built fine | Channels connect lazily; only an invalid URI fails at construction. The host or port is wrong, or the server is down | Check the endpoint and that the server is listening |
| A long-lived stream keeps working after the token expired | Expiry is checked only when the call starts | Re-check `ctx.is_expired()` in the handler |

## The client will not build or run

| Symptom | Cause | Fix |
|---|---|---|
| `ChannelError::Runtime` from `public` / `authenticated` / `get_channel` | No Tokio runtime (checked on every call) | Create clients inside `#[tokio::main]` or `#[tokio::test]` |
| `ChannelError::Transport` | The endpoint is not a valid URI (for example no `http://`) | Fix the string; unreachable hosts are not reported here |
| Changing `RUST_GRPC_LIB_KEEP_ALIVE_*` has no effect | Read once when an endpoint's channel is first created; the pool never evicts | Set the variables before the first client for that endpoint |

## The server will not start or stop cleanly

| Symptom | Cause | Fix |
|---|---|---|
| `ServeError::Bind`: "failed to bind the listen address: Address already in use" (or "Permission denied") | Another process holds the port, or the port needs privileges | Pick a free port or stop the other process. In tests bind port `0` and use `serve_on_listener` (it cannot return `Bind`) |
| `ServeError::Signal`: "failed to register the shutdown signal handlers: .." | `serve` could not register SIGTERM/SIGINT (Ctrl-C on Windows) handlers | Read the I/O error in the message. If your app owns shutdown, use `serve_with_shutdown` or `serve_on_listener`, or `.without_signal_handling()` |
| Panic about a missing reactor or disabled I/O when serving | The Tokio runtime has no I/O driver | Use `#[tokio::main]` or `Builder::enable_all()` |
| The process is killed with SIGTERM or SIGINT instead of draining | `serve_with_shutdown` and `serve_on_listener` install no signal handlers, and `.without_signal_handling()` disables them for `serve` | Use plain `serve(addr)`, or call `server::shutdown_signal()` in your own trigger |
| Shutdown takes about 30 seconds | A connection has an open RPC, often a health `Watch` stream or a slow handler; the default drain budget is 30 seconds, after which the server logs `warn` "shutdown grace expired; closing remaining connections" | Close streams when finished; lower `.shutdown_grace(d)`. Keep your platform's termination deadline above grace plus a possible 5 s telemetry flush (see [Graceful shutdown](server.md#graceful-shutdown)) |
| Load balancer still sends traffic during shutdown | It does not watch `grpc.health.v1.Health`, which reports `NotServing` as soon as shutdown begins | Probe `Check` on `""` (or the service name) |
| `KeyValidatorConfig::from_env()` fails: "environment variable AUTH_JWKS_FILE or AUTH_PEM_FILE not set" | No key source is configured (`AUTH_JWKS_URL` is read only with the `jwks-url` feature) | Set `AUTH_JWKS_URL` (and enable `jwks-url`), `AUTH_JWKS_FILE` or `AUTH_PEM_FILE`. See [Environment variables](server.md#environment-variables) |
| `KeyValidator::new` fails: "failed to read key file: .." or "failed to parse PEM key: .." | The key file is missing, unreadable, or not valid JWKS/PEM | Check the path, permissions and format |
| No logs, spans or events at all | The library never installs a `tracing` subscriber | Install one at startup, for example `tracing_subscriber::fmt::init()`. See [Tracing and logs](server.md#tracing-and-logs) |
| The `rpc` span shows no fields in the output | The subscriber does not print span fields unless spans close or events occur inside them | Enable `FmtSpan::CLOSE`, or log inside the handler |

## Metrics are missing or wrong

| Symptom | Cause | Fix |
|---|---|---|
| No metrics reach the collector | Export is off: no endpoint, `OTEL_SDK_DISABLED=true`, or exporter setup failed. A working `.on_call_complete` observer does not prove export is active | Set `OTEL_EXPORTER_OTLP_METRICS_ENDPOINT` (or `OTEL_EXPORTER_OTLP_ENDPOINT`). Look for an `error` on target `rust_grpc_lib::telemetry`: "failed to start telemetry; using a no-op meter" with reason `invalid OTLP/HTTP endpoint` (not an absolute `http`/`https` URL) or `could not build OTLP/HTTP exporter`. Send an RPC and wait at least one export interval (10 s by default) |
| The collector rejects the data or sees nothing | The endpoint speaks OTLP/gRPC, or the URL path is wrong | Use OTLP/HTTP with protobuf; the metrics path is normally `/v1/metrics`. Startup never checks reachability |
| Warning "invalid duration buckets; using defaults" | `GRPC_DURATION_BUCKETS` or `duration_bucket_bounds_seconds` is empty, non-numeric, non-positive, or not strictly increasing | Use ascending positive seconds, for example `0.005,0.05,0.5` |
| Warning "invalid GRPC_RUNTIME_PROBE; using default" | The value is not `true` or `false` | Fix or unset it |
| `runtime.scheduling_delay` is missing | No active provider (see the first row), or the probe is disabled | Check `GRPC_RUNTIME_PROBE` and `runtime_scheduling_probe(..)` |
| `rpc.method` is `_OTHER` | Unknown path or wrong casing; a probe or scanner hitting the server. Or a policy that does not map to exactly one wire method, logged as `warn` "policy does not identify exactly one wire method; omitting telemetry label" with `reason` `missing` or `ambiguous` | Call with the exact wire name. For the warning, check the service trait and generated code come from the same proto, and that you use the generated server type |
| Final metrics lost at exit | A caller-owned `Telemetry` was never shut down, or the process ended before the flush | `telemetry.shutdown().await` after serving, even when `serve` returned an error. See [Provider ownership](server.md#provider-ownership) |
| Warning "telemetry shutdown timed out after five seconds" or "telemetry flush or shutdown failed" | The collector is slow or unreachable at exit | Check the collector; the data in that last interval may be lost |
| Metric label changes between versions | `rpc.response.status_code` is numeric and `error.type` is the tonic code name, unlike OpenTelemetry's string conventions | Query by number or tonic name as documented in [Built-in metrics](server.md#built-in-metrics) |

## Compile errors

### Macro compile errors

The complete list is under "Compile errors" in the `access_controlled_service` rustdoc. The common ones:

| Message (abridged) | Fix |
|---|---|
| `#[access_controlled_service]` must be placed above `#[async_trait]` | Swap the two attributes |
| method `x` declares no access policy; add `#[roles(..)]` or `#[any_authenticated]` or `#[public]` | Add a policy |
| a method declares exactly one access policy | Remove the extra policy |
| expected `any` or `all` / expected a string literal or a path to a `&str` constant / at least one role is required | Write `#[roles(any("a", ..))]` or `#[roles(all(..))]` with literals or `&str` constants, never empty |
| the request parameter must be `AuthenticatedContext<T>`, not a plain `tonic::Request<T>` | Take `ctx: AuthenticatedContext<T>` (or `PublicContext<T>` for `#[public]`) |
| a `#[public]` method takes `PublicContext<T>`, not `AuthenticatedContext<T>` (or the reverse) | Match the context to the policy |
| `AuthenticatedContext` needs a message type argument | Write `AuthenticatedContext<MyRequest>` |
| the `AuthenticatedContext` parameter must be a plain binding | Bind it as `ctx` or `mut ctx`, not a pattern |
| method `getuser` collides with `get_user` | Rename one; the gate ignores case and `_` |
| method `ping` collides with `ping`, with a note about `#[cfg]` | The macro runs before `#[cfg]` is evaluated, so it sees every `cfg` variant of a method even if only one is compiled in. Write one `impl` block per variant, put `#[cfg(..)]` on each block above `#[access_controlled_service]`, and give each variant its own policy |
| `#[roles]` / `#[public]` / `#[any_authenticated]` has no effect here | Put it on a method inside an annotated `impl` block, or remove it |
| `attempting to skip non-existent parameter` from `#[tracing::instrument(skip(ctx))]` | The macro turns the context into a local variable in the body and renames the parameter, so `ctx` is not a parameter when `instrument` runs. Use `#[tracing::instrument(skip_all)]` and record the fields you want |
| unknown argument; the only argument is `server = <path>` | Use `#[access_controlled_service(server = path::to::FooServer)]` or no argument |
| `E0277`: `X` has no access policy for the generated server `FooServer<X>` (at `add_service`) | Annotate the `impl`, or make `server = ..` match the constructor passed to `add_service` |
| `E0425`: cannot find type `AuthenticatedContext` in this scope | Import it from `rust_grpc_lib::auth`, and enable the `server` feature |
| cannot construct `AuthenticatedContext<_>` with struct literal syntax | In tests use `AuthenticatedContext::for_test` |

### Build and dependency errors

| Symptom | Cause | Fix |
|---|---|---|
| `access_controlled_service`, `GrpcServer`, `telemetry` or `build_support` not found | Features are opt-in | `server` to serve, `build` on the `[build-dependencies]` copy, `test-util` in `[dev-dependencies]` |
| Generated code or macros fail to resolve `::rust_grpc_lib` | The dependency was renamed (`package = ".."`) | Keep the name `rust-grpc-lib` |
| `cannot find ... tonic` / `async_trait` / `tonic_health` after upgrading to 8.0 | The crate no longer expects you to depend on them | Use `rust_grpc_lib::tonic::..`, `#[rust_grpc_lib::async_trait]` and `testing::health_status` / `health_watch`; see the [CHANGELOG](../CHANGELOG.md) migration guide |
| Mismatched types such as "expected `tonic::Status`, found `tonic::Status`", or `prost::Message` not implemented | Your crate has its own `tonic` or `prost` dependency at a different version, so the same names are different types | Remove it and use `rust_grpc_lib::tonic` / `rust_grpc_lib::prost` everywhere you touch generated code or this crate |
| `E0592`: duplicate definitions with name `public` (or `authenticated`) | An RPC's snake_case name collides with a generated client constructor | Rename the RPC in the `.proto` file |
| An RPC or message you need is not generated | Protos only come from [interface-definitions](https://github.com/fermi-ad/interface-definitions); you cannot add your own | Open a PR there, run the "Bump Interface Definitions" workflow, then bump your `tag`. See [Generating the protos](server.md#generating-the-protos) |
| A `PROTOC` you set is ignored | `generate_protos` overwrites it with the vendored binary | Nothing to fix |
| Private git dependency fetch fails | Cargo's built-in git cannot use your credentials | Set `[net] git-fetch-with-cli = true` in `.cargo/config.toml` |
| TLS settings or certificates seem necessary | The infra team handles TLS, not this library | Do not enable tonic TLS features or configure TLS yourself |

## Tests

| Symptom | Cause | Fix |
|---|---|---|
| A handler unit test passes but the policy is wrong | `authenticated_request` bypasses the gate | Add a live-server test with `TestServer` and `assert_policy_table_conforms`. See [Testing](testing.md#check-policies-conformance) |
| `capture_spans` records nothing | The test runs on a `multi_thread` runtime; the subscriber is thread-local | Use the default current-thread `#[tokio::test]`, and keep the returned guard alive |
| `TestServer::shutdown` panics "did not shut down gracefully within 5s; check for open RPC streams" | A stream (such as a health `Watch`) or a slow handler is still open | Drop open streams first, or raise `.shutdown_timeout(d)` |
| `TestServer` start panics "was not ready within 5s" / "stopped before readiness" | The server failed to start or the runtime is blocked | Run inside `#[tokio::test]` with I/O and time enabled; read the panic text for the cause |
| `server.metrics()` panics "TestServer metrics require .with_metrics()" | Metrics were not selected | Use `TestServer::builder(kid).with_metrics()` |
| `wait_for_call_count` / `wait_until` panics with "timed out .. recorded series: .." | No matching call completed within five seconds (or the condition never held) | Compare the listed series with your expected label and status code; remember the harness's startup health checks are recorded too |
| `assert_policy_table_conforms` panics "service conformance failed for N reason(s)" or "the policy table is empty" | The server does not enforce what the table declares (one line per mismatch), or the annotated block has no methods | Read each reason line; make sure you test the service served through `GrpcServer` |
