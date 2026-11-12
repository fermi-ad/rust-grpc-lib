# Troubleshooting

See [Concepts](concepts.md) for how the pieces fit.

## Macro compile errors

The complete list is under "Compile errors" in the `access_controlled_service` rustdoc. The most common:

| Message (abridged) | Fix |
|---|---|
| `#[access_controlled_service]` must be placed above `#[async_trait]` | Swap the two attributes |
| method `x` declares no access policy | Add `#[roles(..)]`, `#[any_authenticated]` or `#[public]` |
| a method declares exactly one access policy | Remove the extra policy |
| expected `any` or `all` / expected a string literal or a path to a `&str` constant | Write `#[roles(any("a", ..))]` or `#[roles(all(..))]` with literals or `&str` constants |
| the request parameter must be `RequestContext<T>`, not a plain `tonic::Request<T>` | Take `ctx: RequestContext<T>` (or `PublicContext<T>` for `#[public]`) |
| a `#[public]` method takes `PublicContext<T>`, not `RequestContext<T>` | Match the context to the policy |
| `RequestContext` needs a message type argument | Write `RequestContext<MyRequest>` |
| a `RequestContext` parameter must be a plain binding | Bind it as `ctx` or `mut ctx` |
| method `getuser` collides with `get_user` | Rename one; the gate ignores case and `_` |
| `#[roles]` / `#[public]` / `#[any_authenticated]` has no effect here | Put it on a method in an annotated `impl`, or remove it |
| A policy error on another crate's attribute, such as `#[other_crate::public]` | Inside an annotated block, the macro matches policy attributes by their last path segment. Don't use other crates' attributes named `roles`, `any_authenticated` or `public` on methods in the block |
| `E0277`: `X` has no access policy for the generated server `FooServer<X>` (at `add_service`) | Annotate the `impl`, or make `server = ..` match the constructor passed to `add_service` |
| cannot find type `RequestContext` in this scope | Import it from `rust_grpc_lib::auth` and enable the `server` feature |
| cannot construct `RequestContext<_>` with struct literal syntax | In tests, use `RequestContext::for_test` |

## Build and setup

| Symptom | Cause | Fix |
|---|---|---|
| `E0592`: duplicate definitions with name `public` (or `authenticated`) | An RPC's snake_case name collides with a `GrpcClient` constructor | Rename the RPC in the `.proto` file; see [Generated clients](client.md#generated-clients) |
| `access_controlled_service`, `GrpcServer` or `build_support` not found | Features are opt-in | `server` to serve, `build` on the `[build-dependencies]` copy, `test-util` in `[dev-dependencies]` |
| Generated code fails to resolve `::rust_grpc_lib` | The `rust-grpc-lib` dependency was renamed (`package = ".."`) | Keep the name `rust-grpc-lib`; see [Adding the dependency](server.md#adding-the-dependency) |
| `cannot find ... tonic` / `async_trait` / `tonic_health` after upgrading to 8.0 | The crate no longer expects you to depend on them | Use `rust_grpc_lib::tonic::..`, `#[rust_grpc_lib::async_trait]` and `rust_grpc_lib::testing::health_status` / `health_watch`; see the [CHANGELOG](../CHANGELOG.md) migration notes |
| Mismatched types such as `expected tonic::Status, found tonic::Status` (or `Request`, `Channel`, `prost::Message` not implemented) | Your crate has its own `tonic` or `prost` dependency at a different version from the one `rust-grpc-lib` re-exports, so the same names are different types | Drop your direct `tonic` / `prost` dependency and use `rust_grpc_lib::tonic` / `rust_grpc_lib::prost` types everywhere you talk to generated code or this crate |
| No logs, spans or events appear | The library never installs a `tracing` subscriber | Install one at startup with `tracing-subscriber`; see [Tracing and logs](server.md#tracing-and-logs) |
| A `PROTOC` you set is ignored | `generate_protos` overwrites it with the vendored binary | Nothing to fix; this is by design |
| I need an RPC or message that is not generated | Protos only come from [interface-definitions](https://github.com/fermi-ad/interface-definitions); you cannot add your own or select a subset | Open a PR in `fermi-ad/interface-definitions`, run the "Bump Interface Definitions" workflow yourself, then bump the `tag` in your `Cargo.toml` once released; see [Generating the protos](server.md#generating-the-protos) |
| TLS settings or certificates are needed | The infra team handles TLS, not this library | Do not enable tonic's TLS features or configure TLS yourself |

## Runtime

| Symptom | Cause | Fix |
|---|---|---|
| Client constructor returns `ChannelError::Runtime` | No Tokio runtime (checked on every call) | Build clients inside `#[tokio::main]` or `#[tokio::test]` |
| Constructor succeeds, first RPC fails (typically `Unavailable`) | Connections are lazy; only an invalid URI fails at construction | Check the host and port |
| `PermissionDenied "required role not present"` with a valid token | The token lacks the realm role; client roles never count | Grant the realm role, or change the policy |
| `PermissionDenied "method not permitted"` | Path not `/<service>/<method>`, or the method is not in the policy table (for example, a method produced by a macro invocation inside the block) | Check the client's service and the `server = ..` argument; write the method as a plain `fn` with a policy |
| `Unimplemented` | The server doesn't serve that service at all, so no gate is reached | Check the endpoint and that the service is added with `add_service` |
| Error text printed twice in an `anyhow` chain | `ServeError` / `ChannelError` include the inner error in their message and in `source()` | Cosmetic; print with `{}` instead of `{:#}` if it bothers you |
| `Unauthenticated "missing Authorization header"` | No token, or not `Bearer ` (case-sensitive); for example a `public` client calling a gated method | Use `authenticated`, or `ctx.forward(..)`; see [Generated clients](client.md#generated-clients) |
| `Internal "authentication is not configured"` | A context was built without the gate: service not served via `add_service`, or a unit test used `Request::new` | Serve via `GrpcServer`; in tests, use `authenticated_request` or `public_request` |
| Client gets `Cancelled` | The server's `.timeout()` elapsed and the handler was dropped | Raise the timeout or speed up the handler |
| A long-lived stream keeps running after the token expired | Expiry is checked only when the call starts | Re-check `ctx.is_expired()` in the handler |
| Keep-alive variable change has no effect | Read once, when an endpoint's channel is created; the pool never evicts | Set it before the first client for that endpoint |
| Graceful shutdown hangs | Open streams, including health `Watch`, delay it | End streams or bound the shutdown with a timeout |
| No `rpc` span fields in logs | The subscriber does not log span close events | Enable `FmtSpan::CLOSE`, or log inside the span; see [Tracing and logs](server.md#tracing-and-logs) |
| `capture_spans` records nothing | The test uses a `multi_thread` runtime | Use the default current-thread `#[tokio::test]` |
| A passing handler unit test, but the policy is wrong | `authenticated_request` bypasses the gate | Add a live-server test and `assert_policy_table_conforms`; see [Testing](testing.md#conformance-checks) |
