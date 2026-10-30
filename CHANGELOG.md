# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [7.8.0] — 2026-10-30

### Added

- **`test-util` feature** (off by default; for `[dev-dependencies]`) enabling the new `rust_grpc_lib::testing` module. It turns on `auth` and `rust-auth-lib`'s `test-fixtures`, and adds an optional `tonic-prost` dependency for the conformance helpers.
  - `testing::KeycloakClaimsBuilder` and `testing::test_fixtures` are re-exported from `rust-auth-lib`, so consumers can build `KeycloakClaims` with any subject, expiry and roles and mint signed tokens without a direct `rust-auth-lib` dependency.
  - `RequestContext::for_test(claims, message)` builds a context for a handler that takes `RequestContext<T>`. It goes through `try_from_request`, so the result has the shape a handler sees in production (empty metadata, no remote address, no `rpc_method`, claims still in the extensions).
  - `testing::with_claims(request, claims)` and `testing::request_with_claims(message, claims)` inject claims into a `tonic::Request`, for tests of handlers that still take `Request<T>` or are guarded by `#[keycloak_authenticated_service]`.
  - `testing::assert_all_rpcs_require_auth(channel, &paths)` calls each `"/package.Service/Method"` path with no token and with a malformed token and asserts `Unauthenticated` for both.
  - `testing::assert_all_rpcs_require_role(channel, &paths, token)` asserts `PermissionDenied` for a valid token that lacks the required role, for services that declare roles.
  - `testing::call_rpc(channel, path, token)` is the primitive behind both helpers, for custom checks.
  - The helpers take an explicit list of paths because the library cannot enumerate a consumer's RPCs. They panic on an empty or malformed list and refuse `grpc.health.v1.Health` and `grpc.reflection.*` paths, which are unauthenticated by design. Failures are reported together, naming each path, the expected code and the actual status.
- **Tests:** unit tests for the injection helpers, `for_test` and the path validation, and live-server tests (both `wrap_service` and `with_interceptor` wirings) showing that the helpers pass on a correctly wired server, flag a server with no auth, flag a role check that lets the caller through, and refuse the health service.

### Changed

- The `integration_tests` crate now builds claims with `KeycloakClaimsBuilder` instead of deserializing hand-written JSON.

### Compatibility

Additive. Nothing changes for a consumer that does not enable `test-util`.

---

## [7.7.0] — 2026-10-29

### Added

- **`RequestContext` parameters in `#[keycloak_authenticated_service]`.** A method whose first non-`self` parameter has a type whose last path segment is `RequestContext` now receives a context built by `RequestContext::try_from_request`. The emitted signature is rewritten to `tonic::Request<T>` under a hygienic name, so it still matches the generated trait. The role guard reuses `ctx.claims` rather than looking the claims up again. Methods that keep `Request<T>` behave as before; both forms can be mixed in one impl. Streaming requests (`RequestContext<Streaming<Req>>`) are supported.
- **`#[keycloak_authenticated_service(claims = SomeClaims)]`** selects the claims type that `Request<T>` methods read from the extensions. Defaults to `KeycloakClaims`. The type must implement `Claims` and provide `has_role(&self, &str) -> bool`.
- **Authorization-denial logging.** A denied call now emits a `tracing::warn!` event with target `rust_grpc_lib::authz` and message `authorization denied`, with the structured fields `user` (the `sub` claim), `rpc.service`, `rpc.method`, `required_roles`, `role_mode` (`any` or `all`) and `remote_addr` when known. Neither the token, nor the other claims, nor the roles the caller holds are logged. The client still receives `permission_denied("required role not present")`. Rate limiting is deliberately left to the consumer's subscriber filters.
- **Precise `#[async_trait]` order error.** Placing `#[keycloak_authenticated_service]` below `#[async_trait]` is detected (a method with no `async` and an `'async_trait` lifetime parameter) and reported as `` `#[keycloak_authenticated_service]` must be placed above `#[async_trait]` `` instead of two opaque type errors.
- **Tests:** `RequestContext` rewrite for unary and streaming handlers, role guard through the context, missing claims, mixed `Request<T>`/`RequestContext<T>` impls, `self: Arc<Self>` receivers, `#[async_trait]` below the macro, denial logging captured with a `tracing` subscriber (fields, `any`/`all` modes, no event on success, no leaked roles) and `trybuild` compile-fail tests for error messages and spans.

### Changed

- **Dependency policy for macro consumers.** The expansion names `::rust_grpc_lib`, `::tonic` and `::tracing`, so it uses whatever versions the consuming crate depends on; `rust-grpc-lib` does not re-export or pin them. Consumers must depend on them under their normal names (renaming a dependency with `package = ".."` is not supported). Three couplings remain and are documented on the macro: the consumer needs `tonic` and `tracing` as direct dependencies; and its `tonic` must be semver-compatible with the one `rust-grpc-lib` is built against, because `RequestContext::try_from_request` and the guard exchange `tonic::Request` and `tonic::Status` with the consumer's code.
- **`tracing` is a required dependency.** `rust-grpc-lib` depends on `tracing` unconditionally. The generated guard always contains its `::tracing` calls (the missing-claims `error` and the denial `warn`), and `RequestContext::try_from_request` always logs a missing claims extension.
- The guard's missing-claims error is now `Status::internal("authentication is not configured")` (it used to name `JwtValidationLayer`), with the detail logged server-side by `tracing::error!`. This matches `RequestContext::try_from_request`. The status code is unchanged. The hint no longer reaches the remote client.
- Macro errors are reported with `syn::Error` spans on the offending tokens, and one error per bad method is reported at once. `#[roles]` now rejects an empty role list, trailing tokens and a duplicate `#[roles]` on one method.
- `grpc-macro` bumped to 0.3.0.

### Compatibility

Additive for consumers, with one addition to the macro's requirements: a crate that uses a `#[roles]` guard must now have `tracing` as a direct dependency, because the guard logs through `::tracing`. The macro still needs the `auth` feature of `rust-grpc-lib` for `KeycloakClaims`.

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
