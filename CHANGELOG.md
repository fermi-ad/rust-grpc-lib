# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [8.0.0] — 2026-11-11

This is a breaking release. Read the migration guide below before upgrading from 7.7.0.

The design in one sentence: **attributes own authentication, and one gate enforces it.** What a method requires, including "nothing", is declared on the method with `#[roles(..)]`, `#[any_authenticated]` or `#[public]`. The service macro emits a policy table and no code that checks anything. The new `GrpcServer::add_service` puts the service behind a single gate that does the checking, and is the only way to serve a service.

In 7.7.0, authentication and authorization were wired separately: a JWT layer (`validator_into_layer` or `wrap_service`) validated tokens, and `#[keycloak_authenticated_service]` injected a role check into each `#[roles]` method. A server that forgot the layer compiled and was open, a method without `#[roles]` got no role check, and handlers could read the raw `authorization` header. 8.0.0 closes each of those gaps at compile time or in the gate.

### Changed (breaking)

- **`#[keycloak_authenticated_service]` is replaced by `#[access_controlled_service]`.** There is no alias. The attribute goes on the `impl` of a tonic service trait, above `#[rust_grpc_lib::async_trait]`; placing it below is now a compile error that says so. It takes one optional argument, `server = path::FooServer`, for when the trait is imported by name and the generated server is not in scope under the derived name (the trait path with its last segment `Foo` replaced by `FooServer`).
- **Every method must declare exactly one access policy**: `#[roles(any(..))]` or `#[roles(all(..))]`, `#[any_authenticated]` (any caller with a valid token), or `#[public]` (no token needed). A method with none, or with more than one, is a compile error that names it; all offending methods are reported in one compile. In 7.7.0 a method without `#[roles]` was left untouched and relied on whatever JWT layer the server had.
- **The macro emits a policy table instead of access checks.** It emits `impl ServicePolicy<FooServer<Self>> for Self` with one entry per method, and no role-check code in the handler. The 7.7.0 guard (read `KeycloakClaims` from the extensions, return `Internal` if missing, `PermissionDenied` if the roles do not match) is gone.
- **A single gate enforces access, before any handler runs.** For each call it parses the method from the URI path, looks it up in the table, validates the bearer token and checks the roles. It rejects with `Unauthenticated` (missing or invalid token) or `PermissionDenied` (`"required role not present"`, the same message as 7.7.0). A method that is not in the table, or an unparsable path, is denied with `PermissionDenied` and logged as an `error`. The gate removes the `authorization` header from every call's metadata, public calls included.
- **Servers are built with `GrpcServer` (`server` feature), and only with it.** `GrpcServer::builder(validator).add_service(FooServer::new, my_impl)`. `add_service` requires `T: ServicePolicy<S>` for the server type `S` the constructor returns, so a service the macro did not process, or a constructor that belongs to a different service, does not compile.
- **The 7.7.0 server-side auth API is removed**: `validator_into_layer`, `wrap_service`, `JwtValidationLayer`, `JwtValidationService` and the `auth::layer` module. There is no supported way to put a service behind a hand-wired tonic `Server`; serve through `GrpcServer`, which keeps health open (the reason `wrap_service` existed).
- **Handlers take a context.** `RequestContext<T>` for `#[roles]` and `#[any_authenticated]` methods, `PublicContext<T>` for `#[public]` ones. A plain `tonic::Request<T>` is no longer accepted in an annotated block, and a `RequestContext` on a `#[public]` method is a compile error. The macro builds the context; `RequestContext::try_from_request` and `PublicContext::try_from_request` are `#[doc(hidden)]` and fail closed with `Internal` unless the call came through the gate.
- **`RequestContext` changes shape.**
  - It loses its claims type parameter: `RequestContext<T, C = KeycloakClaims>` becomes `RequestContext<T>`, and the claims are always `KeycloakClaims`. Custom claims types are not supported.
  - Only `claims` and `message` remain fields. `metadata`, `extensions` and `remote_addr` are reached through `Deref` to the new `CallInfo`, read-only (there is no `DerefMut`). `rpc_method: Option<RpcMethod>` becomes `ctx.rpc: RpcMethod`, always present.
  - The validated claims are no longer in the request extensions, and the `authorization` header is no longer in the metadata. Read `ctx.claims`; forward the token with `ctx.token()` or `ctx.forward(..)`.
  - `Debug` prints the token as `<redacted>`.
- **`auth::extract_token` is removed.** Use `ctx.token()`, `ctx.forward(message)` or `ctx.into_forwarded()`.
- **The `auth::context`, `auth::interceptor` and `auth::layer` module paths are no longer public.** Every remaining type is exported directly from `rust_grpc_lib::auth` (for example `rust_grpc_lib::auth::ClientJwtInterceptor`, `rust_grpc_lib::auth::RequestContext`).
- **Client constructors are renamed and always generated.** `#[derive(GrpcClient)]` generates `public(endpoint)`, which sends no token, and `authenticated(endpoint, provider)`, which attaches the provider's token to every request. `from_endpoint_with_provider` becomes `authenticated`. `from_endpoint` and the `GrpcNoAuthClient` derive are removed; `public` replaces them and needs no feature. Because the constructors are inherent associated functions, an RPC whose snake_case name is `public` or `authenticated` collides with them (`E0592`); rename the RPC in the `.proto` file.
- **Client constructors and `pool::get_channel` return `pool::ChannelError`** instead of `tonic::transport::Error`, and return `Err(ChannelError::Runtime(..))` instead of panicking when called outside a Tokio runtime.
- **`build_support::Config::new` always applies `#[derive(GrpcClient)]`**, whatever features the `rust-grpc-lib` build-dependency has. It no longer applies `GrpcNoAuthClient`.
- **Generated code and macro output refer only to `::rust_grpc_lib`.** `#[access_controlled_service]`, `#[derive(GrpcClient)]` and the code `generate_protos` writes go through a `#[doc(hidden)]` `rust_grpc_lib::__private` module (not public API), never `::tonic`, `::prost` or `::tonic_prost`. Code generated by 7.7.0 refers to `::tonic` and `::prost`; a rebuild regenerates it, since `build.rs` reruns.
- **`tonic` and `prost` are part of the public API.** They are re-exported (see Added), so a bump of either to a semver-incompatible version is a breaking change of `rust-grpc-lib`.
- **Features are reorganized.** There is no default feature set (`default = []`).
  - `auth` is removed. Its client half (token sources, `ClientJwtInterceptor`, the `rust-auth-lib` re-exports, the channel pool and `GrpcClient`) is always compiled. Its server half (`RequestContext`, the policy types and the service macros) moves to the new `server` feature. `rust-auth-lib` is no longer optional.
  - `unauthenticated` is removed, because `public` is always generated.
  - `tracing` is removed; `tracing` is always a dependency.
  - `server` (new) enables `GrpcServer`, the contexts, the policy types and the `access_controlled_service`, `roles`, `any_authenticated` and `public` attributes.
  - `test-util` (new) enables `rust_grpc_lib::testing` and the `for_test` constructors, and implies `server`.
  - `build` and `jwks-url` are unchanged.
- **`#[roles]` outside an annotated block is a compile error.** In 7.7.0 it was a silent pass-through that left the method unchecked. `#[any_authenticated]` and `#[public]` behave the same way.

### Added

- **`GrpcServer`** (`server`): `GrpcServer::builder(validator)`, `.timeout(duration)`, `.add_service(FooServer::new, my_impl)`, and three serve methods that share the `#[non_exhaustive]` error `ServeError` (`Bind`, `Transport`):
  - `serve(addr) -> Result<Infallible, ServeError>` runs until the server fails.
  - `serve_with_shutdown(addr, signal) -> Result<(), ServeError>` drains gracefully once `signal` completes.
  - `serve_on_listener(listener, signal) -> Result<(), ServeError>` serves on a listener you bound (useful in tests with port `0`); it never returns `ServeError::Bind`.
- **Health is built in.** Every `GrpcServer` serves `grpc.health.v1.Health` without a token. The overall service (`""`) and every added service report `Serving` once the server starts, and `NotServing` as soon as the shutdown signal fires, before the drain. A consumer no longer needs `tonic-health`.
- **Per-RPC tracing is built in.** Every RPC, including health calls and rejections, gets an `rpc` span on target `rust_grpc_lib::trace` with `rpc.system`, `rpc.service`, `rpc.method`, `grpc.status`, `latency_ms`, `stream_ms`, `access` (`public`, `authenticated` or `roles`) and `user`. The library emits spans and events but never installs a subscriber.
- **A denial event.** A role denial emits a `warn` on target `rust_grpc_lib::authz` with `user`, `rpc.service` (the full service name, for example `services.example.ExampleDB`), `rpc.method` (the proto name, for example `getEndPointOne`), `required_roles`, `role_mode` and `remote_addr`. It never logs the token, other claims or the roles the caller holds.
- **`#[public]` and `PublicContext<T>`**: the one, greppable way to open a method. `PublicContext` has the message and the call details and nothing that implies a verified caller. A public method gets no claims even if the caller sent a token (optional authentication is not supported).
- **`#[any_authenticated]`**: any caller with a valid token, no role needed.
- **Typed roles.** `#[roles(any(..))]` and `#[roles(all(..))]` accept a path to a `&str` constant (`roles::OPERATOR`) as well as a string literal, and the two can be mixed. An empty list is a compile error.
- **`CallInfo`**, which both contexts dereference to: `rpc`, `remote_addr`, `metadata`, `extensions`, `span()` and `instrument(future)`, which runs a future (for example a `tokio::spawn`ed task) inside the RPC span.
- **Forwarding the caller's token.** `ctx.token()` returns a `ForwardedToken` for `FooClient::authenticated(url, ctx.token())`. `ctx.forward(message)` returns a `tonic::Request<U>` with the `authorization` header set and marked sensitive. `ctx.into_forwarded()` does the same with the context's own message, without a clone, and `ctx.into_parts()` returns `(T, ForwardedToken)`. `ForwardedToken` has no `Debug` or `Display`.
- **`ServicePolicy`, `Access`, `MethodPolicy`, `PolicyTable` and `RoleMode`** in `rust_grpc_lib::auth`: the table the macro emits, readable by consumers, for example to print each method's policy at startup. `PolicyTable::for_service(FooServer::<Impl>::new)` reads it from the generated constructor without calling it.
- **A name-collision check.** Two methods whose names are equal once lowercased and stripped of underscores are a compile error, because the gate matches proto names (`getEndPointOne`) against Rust names (`get_end_point_one`) that way.
- **`auth::AuthenticatedClient<P>`**, an alias for `InterceptedService<Channel, ClientJwtInterceptor<P>>`: the transport type of a client built with `authenticated`. Name it to store a client in a struct, for example `ExampleDbClient<AuthenticatedClient<ForwardedToken>>`.
- **`pool::ChannelError`** (`#[non_exhaustive]`: `Runtime(TryCurrentError)`, `Transport(tonic::transport::Error)`), implementing `std::error::Error` with `source()`.
- **The channel pool normalizes its key.** Endpoint strings that differ only by scheme or host case (`HTTP://Host:50051`, `http://host:50051`) or by a lone trailing `/` share one channel. Different ports (default ports are not stripped), userinfo, paths other than `/`, and queries stay separate. The channel connects to the first string seen, and the pool still never evicts entries.
- **`rust_grpc_lib::tonic`, `rust_grpc_lib::prost` and `rust_grpc_lib::async_trait`**: re-exports of the exact `tonic` and `prost` this crate is built against (0.14), and of tonic's `async_trait` attribute. They need no feature.
- **`rust_grpc_lib::testing`** (`test-util`), a new module:
  - Tokens: `test_validator(kid)`, `test_token(kid, sub, roles)`, `expired_test_token(..)`, `claims_with_roles(sub, roles)`, and the `rust-auth-lib` `test_fixtures` / `KeycloakClaimsBuilder` re-exports.
  - Calling a handler directly: `authenticated_request(message, claims)` and `public_request(message)` build the request an annotated handler expects. `RequestContext::for_test(claims, message)` and `PublicContext::for_test(message)` build a context, with `with_rpc`, `with_remote_addr`, `with_metadata` and `with_extension` builders.
  - Live-server conformance: `assert_policy_table_conforms(channel, FooServer::<Impl>::new)` checks every method in the policy table (public methods are reachable without a token; every other method answers `Unauthenticated` to no token and to an invalid one). `assert_service_conforms(channel, constructor, &["methodName", ..])` does the same against a list you write, so it also catches a listed method missing from the table. `assert_all_rpcs_require_auth`, `assert_all_rpcs_require_role` and `call_rpc` work on raw paths.
  - Health: `health_status(channel, service)` and `health_watch(channel, service)`, with `ServingStatus` and a `StreamExt` re-export, so tests need no `tonic-health` or `futures-util`.
  - Spans: `capture_spans()` returns a `SpanCapture` of the `rpc` spans for assertions.
- **Guides** in `docs/`: `concepts.md` (a glossary and architecture overview: the request flow, the gate, the policy table, contexts, claims, realm roles, token providers and validators, and common mistakes), `server.md`, `client.md`, `testing.md` and `troubleshooting.md`. The README is shortened to a quick start that links to them, and to the [service template](https://github.com/fermi-ad/rust-grpc-service-template) for a runnable example.

### Fixed

- **`build_support` escapes package segments exactly as prost-build does.** `proto.rs` wrote `r#` for `union`, `self`, `super`, `crate`, `Self` and `extern`, so a package with one of those segments did not compile (and `union` named a file prost never wrote). It now leaves `union` plain and appends `_` to the others (`demo.self.v1` is `demo::self_::v1`), in both the `mod` names and the `include_proto!` file names. No bundled package uses these names, so the generated output is unchanged.

### Documentation

- **Protos come only from `interface-definitions`.** The guides and the `build_support` rustdoc say why you cannot add your own protos or select a subset (every service and client in the org shares one set of generated types) and how to get a new or changed RPC: open a PR in `fermi-ad/interface-definitions`, run the "Bump Interface Definitions" workflow, then bump your `tag`.
- **TLS is not configured by this library.** Services operate over plaintext and the infra team handles TLS; do not enable tonic's TLS features. `docs/concepts.md` notes that bearer tokens travel unencrypted between the service and the platform's TLS layer, so a service must only be reachable through that layer. The library never logs tokens.
- **Minimal consumer dependencies.** A service needs `rust-grpc-lib` (with `server`), `tokio`, `tracing` and `tracing-subscriber`; a client needs `rust-grpc-lib` and `tokio`; `build.rs` needs `rust-grpc-lib` with `build`; tests need `rust-grpc-lib` with `test-util` plus `tokio`. Troubleshooting has an entry for mismatched tonic types caused by a separate `tonic` dependency at another version.
- rustdoc examples are compiled doctests instead of `rust,ignore` blocks: plain doctests where they can run, `no_run` where they need a server, a network or a generated client.

### Internal

- `grpc-macro` 0.6.0: the single `lib.rs` is split into `client` (the `GrpcClient` derive) and `service` (`args`, `async_trait_order`, `policy`, `request_param`, `roles`), each with unit tests. `rust-grpc-lib` is a dev-dependency for its doctests only.
- `integration_tests` 0.2.0: `integration_round_trip`, `keycloak_authenticated_service_macro` and `per_service_auth` are replaced by `access_gate` (live-server tests of public methods, rejections that never reach the handler, the header scrub, spans and denial logs, an unlisted method, the conformance helpers, and forwarding through a second in-process server), `access_controlled_service_macro`, `method_name_matching`, `server_builder` and `trace_layer`. A new `trybuild` suite covers every compile error above. The crate now stands in for a minimal consumer: it depends on `rust-grpc-lib` (`test-util`, `server`), `tokio`, `tracing`, `tracing-subscriber` and `trybuild` only.
- `scripts/gen-test-fixtures.sh` generates the fixtures through `build_support` with the workspace `Cargo.lock`, so they match what consumers get from `generate_protos`.

### Migration guide (from 7.7.0)

1. **Update `Cargo.toml`.**
   - Remove `"auth"`, `"unauthenticated"` and `"tracing"` from every `rust-grpc-lib` entry; those features no longer exist.
   - A service adds `features = ["server"]`. Tests add `features = ["test-util"]` (in `[dev-dependencies]`). A pure client needs no feature. `[build-dependencies]` keeps `features = ["build"]`.
   - Remove the direct `tonic`, `prost`, `tonic-prost`, `async-trait` and `tonic-health` dependencies (keep `tokio`, `tracing` and `tracing-subscriber`). If you keep your own `tonic` or `prost`, it must resolve to the same version `rust-grpc-lib` re-exports (0.14), or the types will not match.
2. **Switch to the re-exports.** `use tonic::..` becomes `use rust_grpc_lib::tonic::..`, `prost::..` becomes `rust_grpc_lib::prost::..`, and `#[tonic::async_trait]` becomes `#[rust_grpc_lib::async_trait]`. Rebuild to regenerate `proto.rs`; nothing in `build.rs` changes.
3. **Rename the attribute.** `#[keycloak_authenticated_service]` (and `#[rust_grpc_lib::keycloak_authenticated_service]`, and its `use`) becomes `#[access_controlled_service]`. Keep it above `#[rust_grpc_lib::async_trait]`. If the compiler cannot find `FooServer` from the trait path, add `server = path::to::FooServer`.
4. **Give every method a policy.**
   - Methods that had `#[roles(..)]` keep it.
   - Methods without `#[roles]` that sat behind your JWT layer, so any valid token was accepted, get `#[any_authenticated]`.
   - Methods that must be reachable without a token (in 7.7.0, a service you served without the layer or `wrap_service`) get `#[public]`.
   - Helper methods that are not RPC handlers cannot carry a policy; move them to a separate, unannotated `impl` block.
5. **Change the handler parameter to a context.**
   - `request: Request<T>` becomes `ctx: RequestContext<T>`, or `ctx: PublicContext<T>` on a `#[public]` method. `RequestContext<T, KeycloakClaims>` becomes `RequestContext<T>`.
   - Replace `request.into_inner()` with `ctx.message`, and `request.metadata()` / `request.extensions()` / `request.remote_addr()` with `ctx.metadata` / `ctx.extensions` / `ctx.remote_addr`.
   - Delete `let ctx = RequestContext::try_from_request(request)?;` lines; the macro writes that for you.
   - Replace `ctx.rpc_method` (an `Option`) with `ctx.rpc`.
   - Anything that read `KeycloakClaims` from the request extensions reads `ctx.claims` instead (or `ctx.sub()`, `ctx.has_role(..)`).
   - The context is read-only. Copy what you need out of it, for example `let mut metadata = ctx.metadata.clone();`.
6. **Serve with `GrpcServer`.** Replace the hand-built tonic `Server`:
   ```rust
   // 7.7.0
   let (_reporter, health) = tonic_health::server::health_reporter();
   Server::builder()
       .add_service(health)
       .add_service(wrap_service::<KeycloakClaims, _, _>(FooServer::new(MyFoo), validator))
       .serve(addr)
       .await?;

   // 8.0.0
   GrpcServer::builder(validator)
       .add_service(FooServer::new, MyFoo)
       .serve(addr)
       .await?;
   ```
   `validator` is an `Arc<impl TokenValidator>`, for example `Arc::new(KeyValidator::new(config)?)`. Health is added and reported for you; drop your `health_reporter` and `set_serving` calls. A global `Server::builder().layer(validator_into_layer(..))` is replaced the same way. Use `serve_with_shutdown(addr, signal)` for a graceful drain.
7. **Forward credentials with the context.** Replace `extract_token(&request)?` with `ctx.token()`, for example `FooClient::authenticated(url, ctx.token())?`. To pass the caller's message and token on, use `downstream.call(ctx.into_forwarded())`, or `ctx.forward(other_message)` for a different message.
8. **Rename the client constructors.** `FooClient::from_endpoint_with_provider(url, provider)` becomes `FooClient::authenticated(url, provider)`, and `FooClient::from_endpoint(url)` becomes `FooClient::public(url)`. Delete any hand-written `#[derive(GrpcNoAuthClient)]`. Both constructors (and `pool::get_channel`) return `Result<_, pool::ChannelError>`: `?` into `Box<dyn Error>` keeps working, but code that named or matched `tonic::transport::Error` for these calls uses `ChannelError` (an invalid endpoint is `ChannelError::Transport`). A type spelled `FooClient<InterceptedService<Channel, ClientJwtInterceptor<P>>>` can become `FooClient<AuthenticatedClient<P>>`. If an RPC's snake_case name is `public` or `authenticated`, rename it in the `.proto` file.
9. **Update imports from `auth` submodules.** `rust_grpc_lib::auth::context::RequestContext`, `rust_grpc_lib::auth::interceptor::ClientJwtInterceptor` and similar become `rust_grpc_lib::auth::RequestContext`, `rust_grpc_lib::auth::ClientJwtInterceptor`.
10. **Update the tests.**
    - Replace hand-rolled JWT fixtures with `testing::test_validator(kid)` and `testing::test_token(kid, sub, roles)`, and start test servers with `GrpcServer::builder(test_validator(kid))...serve_on_listener(listener, shutdown)`.
    - To call a handler directly, pass `testing::authenticated_request(message, claims)` or `testing::public_request(message)`; a raw `Request` without them is rejected with `Internal`. Code that takes a context directly can use `RequestContext::for_test(claims, message)` or `PublicContext::for_test(message)`.
    - Replace hand-built `HealthClient` checks with `testing::health_status(channel, "")` or `health_watch(..)`.
    - Add `testing::assert_policy_table_conforms(channel, FooServer::<Impl>::new)` to check that the live server enforces the declared policies.

### Compatibility

Breaking, as described above. The status codes a client sees are unchanged: `Unauthenticated` for a missing or invalid token, and `PermissionDenied` with `"required role not present"` for a role mismatch. A call that reached a misconfigured 7.7.0 handler with no claims (`Internal`) cannot happen any more, because an unprotected service does not compile. A consumer now needs only `rust-grpc-lib` (not renamed) plus `tokio`, and `tracing` and `tracing-subscriber` for a service. `rust-grpc-lib` is built on tonic 0.14 and prost 0.14; since both are re-exported, a future bump of either to an incompatible version is a breaking release of this crate.

---

## [7.7.0] — 2026-11-10

### Changed

- **Updated the pin of interface-definitions.** Update mi8-autotune proto

---

## [7.6.0] — 2026-10-29

### Added

- **`RequestContext<T, C: Claims = KeycloakClaims>`** in the new [`auth::context`](crates/core/src/auth/context.rs) module, re-exported from `rust_grpc_lib::auth`. The claims type `C` must implement `Claims` and defaults to `KeycloakClaims`; the bound is declared on the struct and its main `impl`, so `try_from_request` only adds `C: Clone`. It bundles the request message (or `Streaming<Req>`), the validated claims, metadata, extensions, `remote_addr` and an optional `rpc_method`. Build it with `RequestContext::try_from_request(request)`, and read claims with the `sub()`, `has_role()`, `exp_unix()` and `is_expired()` accessors. A missing claims extension fails closed with `Status::internal("authentication is not configured")`; the detail is logged server-side and is not sent to the client.
- **`RpcMethod { service, method }`**, a library-owned type. `RequestContext::rpc_method` is `None` until something inserts an `RpcMethod` extension (no layer in this release does).
- **Per-service auth constructors** in `auth::layer`, so a service can be authenticated without a global layer (for example to keep `tonic-health` open):
  - `JwtValidationService::new(validator)` builds the bare interceptor for a generated server's `with_interceptor`.
  - `wrap_service::<C, V, S>(service, validator)` wraps a generated server without a direct `tower` dependency. The result keeps `NamedService`, so a health reporter's `set_serving::<..>()` still works.
- **`tracing` feature**, enabled by `auth` (and so by default), adds an optional `tracing` dependency used to log misconfiguration server-side.
- **Tests** for the context, the new constructors, and an in-process server with an unauthenticated health service next to an authenticated service (no-token health returns `Serving`, a no-token RPC returns `Unauthenticated`, and a valid token reaches the handler).

### Changed

- `JwtValidationService` now implements `Clone` for any `C` and `V` (only the `Arc` is cloned), and its fields stay private.
- `Server::layer` with `validator_into_layer` still guards every service on the server, health included. There is deliberately no exemption mechanism; use `wrap_service` for services that must stay open. The module docs now say so.

### Fixed

- Doc comment in [`lib.rs`](crates/core/src/lib.rs) named `authenticated_service` instead of `keycloak_authenticated_service`.
- Stale `block_in_place` comment in `JwtValidationService::call`; validation is synchronous.

---

## [7.5.0] — 2026-10-28

### Changed

- **Updated the pin of interface-definitions.** Update mi8-autotune proto

---

## [7.4.0] — 2026-10-22

### Changed

- **Updated the pin of interface-definitions.** Adds the MI8 Autotune interface

---

## [7.3.0] — 2026-10-21

### Changed

- **Updated the pin of interface-definitions.** Adds the BLM `ListAvailableBeamLines` RPC and its request/response messages for querying the static `BeamLine` values available to BLM clients.

---

## [7.2.0] — 2026-10-16

### Changed

- **Updated the pin of interface-definitions.** Adds the order field to the UNR Entity message

---

## [7.1.0] — 2026-10-05

### Changed

- **Updated the pin of interface-definitions.** This routine integration adds the BLM gRPC service and expands `common.device.Value` to support signed and unsigned 64-bit integers, both as individual values and arrays.

---

## [7.0.0] — 2026-09-21

### Breaking Changes

- **Updated the pin of interface-definitions.** The UNR [`ReadRelationshipRequest.ids`](crates/core/interface-definitions/proto/controls/service/grpc-unr/relationship.proto:29) field was renamed from its singular form. Update callers using the generated request field; its Protobuf field number is unchanged.

### Added

- **Automated interface-definition updates.** Added an [update script](scripts/update-interface-definitions.sh) for major and minor version bumps and a [GitHub Actions workflow](.github/workflows/bump-interface-definitions.yaml) that opens an update pull request. The [README](README.md) documents both workflows.

---

## [6.0.0] — 2026-09-09

### Breaking Changes

- **Updated the pin of interface-definitions.** Reworked the UNR interfaces: the old BaseInfo and RelationshipInfo services and their versioned Protobuf files were replaced by [`EntityService`](crates/core/interface-definitions/proto/controls/service/grpc-unr/entity.proto:7) and [`RelationshipService`](crates/core/interface-definitions/proto/controls/service/grpc-unr/relationship.proto:7). Migrate generated clients and request/response types to the new services and packages.

---

## [5.0.0] — 2026-09-01

### Changed  

- **Updated the pin of interface-definitions.** The latest commit to main removes a duplicated `repeated` term in the UNR `RelationshipResponse` message definition.

---

## [4.1.0] — 2026-08-24

### Changed  

- **Updated the pin of interface-definitions.** The latest commit to main includes the `WScan.proto` file that used to live in `extapi-acsys`. It is now exposed here.

---

## [4.0.0] — 2026-08-18

### Breaking Changes

- **`pool::get` removed.** The old `pool::get::<C>(endpoint)` function, which returned a typed client directly, has been replaced by [`pool::get_channel(endpoint)`](crates/core/src/pool.rs) which returns a raw `tonic::transport::Channel`. Client construction is now handled by the `from_endpoint_with_provider` / `from_endpoint` constructors generated by the new derive macros. Update call sites:

  ```rust
  // Before (v3)
  let client: AlarmCommandsClient<_> = pool::get("http://host:50051")?;

  // After (v4)
  let client = AlarmCommandsClient::from_endpoint_with_provider("http://host:50051", provider)?;
  ```

- **`GrpcClient` trait removed.** The `rust_grpc_lib::GrpcClient` trait (with its `from_channel` method) no longer exists. `GrpcClient` is now a derive macro (see Added below). Remove any manual `impl GrpcClient` blocks and any code that calls `from_channel`.

- **`auth` feature is now on by default.** Previously the library had no default features. Now `default = ["auth"]`. If you were depending on the library without auth and want to keep it that way, add `default-features = false` to your dependency declaration.

- **`grpc-client-macro` crate renamed to `grpc-macro`.** The internal proc-macro crate has been renamed. This is not a direct consumer-facing break (it was always re-exported from the main crate), but any workspace that pinned the internal crate directly must update the name.

### Added

- **Zero-trust JWT authentication** (`auth` feature, on by default). New module `rust_grpc_lib::auth` providing:
  - [`ClientJwtInterceptor`](crates/core/src/auth/interceptor.rs) — outbound interceptor that attaches `Bearer` tokens to every gRPC request. The `provider` field is private; use `ClientJwtInterceptor::new(provider)` to construct. Header parse errors are propagated as `Status::internal` rather than panicking.
  - [`JwtValidationLayer<C, V>`](crates/core/src/auth/layer.rs) / [`JwtValidationService<C, V>`](crates/core/src/auth/layer.rs) — server-side tower layer that validates incoming JWTs and rejects unauthenticated requests before they reach any handler. Generic over both the validator `V: TokenValidator` and the claims type `C: Claims`, so it works with any claims shape — not just `KeycloakClaims`. For the standard Keycloak setup: `validator_into_layer::<KeycloakClaims, _>(validator)`.
  - [`validator_into_layer`](crates/core/src/auth/layer.rs) — constructor for `JwtValidationLayer`.
  - [`extract_token`](crates/core/src/auth.rs) — strips the `Bearer ` prefix from an incoming request's `Authorization` header and returns a `ForwardedToken`.
  - Re-exports of `rust-auth-lib` types: `TokenProvider`, `TokenValidator`, `Claims`, `Ephemeral`, `FileTokenProvider`, `ForwardedToken`, `KeyValidator`, `KeyValidatorConfig`, `KeycloakClaims`, `AuthError`, `ConfigError`, `TokenError`.

- **`#[derive(GrpcClient)]` macro.** Generates a `from_endpoint_with_provider(endpoint, provider)` constructor on any tonic client struct. Applied automatically by `Config::new()` to every generated client. Re-exported as `rust_grpc_lib::GrpcClient`.

- **`#[derive(GrpcNoAuthClient)]` macro** (`unauthenticated` feature). Generates a `from_endpoint(endpoint)` constructor with no auth wiring. For test harnesses only. Re-exported as `rust_grpc_lib::GrpcNoAuthClient`.

- **`#[keycloak_authenticated_service]` and `#[roles(...)]` attribute macros.** Apply `#[keycloak_authenticated_service]` to an `impl Trait for Type` block to inject Keycloak role-checking guards into methods annotated with `#[roles(any("r1", "r2"))]` or `#[roles(all("r1", "r2"))]`. Re-exported as `rust_grpc_lib::keycloak_authenticated_service`.

- **`unauthenticated` feature flag.** Enables `GrpcNoAuthClient` and the `from_endpoint` constructor on generated clients. Intended for test harnesses only; do not enable in production.

- **`jwks-url` feature flag.** Enables live JWKS endpoint rotation via `KeyValidatorConfig::from_jwks_url` (opt-in; pulls in an HTTP client via `reqwest`).

- **Integration test crate** (`crates/integration_tests`). Exercises the full client→server gRPC round-trip with real JWT auth, real tonic transport, and the `#[keycloak_authenticated_service]` macro expansion. Tests run with `cargo test` and require no live `protoc` invocation — fixtures are pre-generated and committed.

- **`scripts/gen-test-fixtures.sh`** — script to regenerate committed proto fixtures after updating the `interface-definitions` submodule.

- **`Config::new()` now conditionally applies derives** based on active features. When `auth` is enabled, `#[derive(GrpcClient)]` is applied to every generated client. When `unauthenticated` is enabled, `#[derive(GrpcNoAuthClient)]` is also applied.

### Changed

- **Tests moved to dedicated files.** Inline `#[cfg(test)] mod tests { … }` blocks have been extracted to separate `tests.rs` files (e.g. `pool/tests.rs`, `auth/tests.rs`, `auth/layer/tests.rs`, `build_support/file_utils/tests.rs`) for easier navigation.

- **`build_support.rs` doc comments expanded.** `Config::new()` now documents all baseline settings it applies to the underlying `tonic_prost_build::Builder`.

- **README rewritten.** The README now documents all three service archetypes (middle-layer, edge/hardware, GraphQL gateway), all feature flags, all environment variables, the proc-macro reference, and the integration test fixture workflow.

- **CI updated to `rust-integration.yaml@swb_v2`** with coverage exclusions for integration test files and `tests.rs` modules.

- **Dependabot updated** to track `devcontainers` ecosystem and switched GitHub Actions and devcontainers schedules to monthly.

- **Dev container updated** to Rust `1.97.1`.

---
