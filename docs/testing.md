# Testing your service

See [Concepts](concepts.md) for how the pieces fit. For a runnable service with a test suite, see the [service template](https://github.com/fermi-ad/rust-grpc-service-template).

The helpers live in `rust_grpc_lib::testing` (`test-util` feature, which implies `server`). The module's rustdoc lists every helper.

> [!WARNING]
> Enable `test-util` in `[dev-dependencies]` only. It ships a publicly known RSA key, and `test_validator` trusts anything signed with it. Nothing stops a release build from enabling the feature. A server built with `test_validator` in production lets **anyone** authenticate as any user with any role.

Tests need `rust-grpc-lib` with `test-util` in `[dev-dependencies]`, plus `tokio` (for `#[tokio::test]`). Nothing else: tonic types come from `rust_grpc_lib::tonic`, health queries from `testing::health_status` / `health_watch`, and `testing::StreamExt` reads the `Watch` stream, so no `tonic`, `tonic-health` or `futures-util` dependency is needed.

| Kind of test | Helpers |
|---|---|
| [Handler unit tests](#handler-unit-tests) | `claims_with_roles`, `KeycloakClaimsBuilder`, `authenticated_request`, `public_request`, `RequestContext::for_test`, `PublicContext::for_test` |
| [Live-server tests](#live-server-tests) | `test_validator`, `test_token`, `expired_test_token` |
| [Health checks](#health-checks) | `health_status`, `health_watch`, `ServingStatus` |
| [Conformance checks](#conformance-checks) | `assert_policy_table_conforms`, `assert_service_conforms`, `assert_all_rpcs_require_auth`, `assert_all_rpcs_require_role`, `call_rpc` |
| [Capturing spans](#capturing-spans) | `capture_spans`, `SpanCapture`, `SpanRecord` |

The examples below use a service `ExampleDB` (`package services.example;`) implemented by `ExampleDbService`; substitute your own generated types.

## Handler unit tests

After the macro rewrite, handlers take a `rust_grpc_lib::tonic::Request<T>`. These helpers build one carrying what the gate would add, so a test can call the handler directly. The gate is **bypassed**: this tests handler logic, not access policy.

```rust
use rust_grpc_lib::testing::{authenticated_request, claims_with_roles, public_request};

let req = authenticated_request(IdList { id: vec![7, 8] }, claims_with_roles("alice", &["operator"]));
let reply = ExampleDbService.get_end_point_one(req).await.unwrap().into_inner();

// A #[public] method takes a request built by public_request.
let reply = MyService.my_public_method(public_request(Empty {})).await.unwrap().into_inner();
```

- A plain `Request::new`, or `public_request` passed to an authenticated handler, gives `Internal "authentication is not configured"`.
- `authenticated_request` carries the literal token `"test-token"`, not a JWT. A handler that forwards `ctx.token()` to a real downstream service therefore sends a token that the downstream rejects with `Unauthenticated`. Test forwarding with a live server (below).
- `claims_with_roles(sub, roles)` covers the common case; `KeycloakClaimsBuilder` builds custom claims (expiry, issuer, client roles).
- For logic that takes a context rather than a request, build one with `RequestContext::for_test(claims, msg)` or `PublicContext::for_test(msg)`.
- Contexts are read-only (no `DerefMut`). To simulate more call details, chain `with_rpc(RpcMethod)`, `with_remote_addr(addr)`, `with_metadata(map)` or `with_extension(value)` onto `for_test`: `PublicContext::for_test(msg).with_metadata(map)`.

## Live-server tests

Run the real server on a free port with a validator that trusts test tokens, then call it with a generated client:

```rust
use rust_grpc_lib::auth::ForwardedToken;
use rust_grpc_lib::server::GrpcServer;
use rust_grpc_lib::testing::{test_token, test_validator};

const KID: &str = "my-test-kid";

#[tokio::test]
async fn operator_can_read() -> Result<(), Box<dyn std::error::Error>> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let (_stop, rx) = tokio::sync::oneshot::channel::<()>(); // dropping _stop shuts the server down
    tokio::spawn(
        GrpcServer::builder(test_validator(KID))
            .add_service(ExampleDbServer::new, ExampleDbService)
            .serve_on_listener(listener, async { let _ = rx.await; }),
    );

    let token = test_token(KID, "alice", &["operator"]);
    let mut client = ExampleDbClient::authenticated(&format!("http://{addr}"), ForwardedToken::new(token))?;
    let reply = client.get_end_point_one(IdList { id: vec![1, 2] }).await?.into_inner();
    // ... assert on reply ...
    Ok(())
}
```

- `test_token(kid, sub, roles)` is valid for one hour; `expired_test_token(kid, sub, roles)` expired two minutes ago. `test_validator(kid)` rejects tokens minted with another `kid`.
- `ExampleDbClient::public(..)` sends no token: a `#[public]` method answers normally, and a gated method answers `Unauthenticated`.
- A garbage or expired token also gives `Unauthenticated`; a valid token missing the role gives `PermissionDenied "required role not present"`.
- The listener is bound before the server task is spawned, so early connections queue and no startup delay is needed. Each test binds its own port, so tests run in parallel.
- To test forwarding, start the downstream and the gateway on separate ports with the same `test_validator(KID)`, point the gateway at the downstream's address, and call the gateway with a token. Each service enforces its own policy on the forwarded token.

## Health checks

`GrpcServer` serves `grpc.health.v1.Health` automatically, and two helpers query it without a token, so tests need no `tonic-health` dependency:

```rust
use rust_grpc_lib::testing::{ServingStatus, StreamExt, health_status, health_watch};
use rust_grpc_lib::tonic::transport::Channel;

let channel = Channel::from_shared(format!("http://{addr}"))?.connect().await?;

// One Check: "" is the whole server, SERVICE_NAME one service.
assert_eq!(health_status(channel.clone(), "").await?, ServingStatus::Serving);

// A Watch stream: the current status first, then each change.
let mut statuses = health_watch(channel, SERVICE_NAME).await?;
let first = statuses.next().await.unwrap()?;
```

- `health_status(channel, service)` returns `Result<ServingStatus, Status>`. `health_watch(channel, service)` returns `Result<impl Stream<Item = Result<ServingStatus, Status>>, Status>`; read it with `testing::StreamExt` (a re-export of `futures_util::StreamExt`, for `next` and the other combinators), so you need no `futures-util` dependency.
- `ServingStatus` is the `grpc.health.v1` protobuf enum (`Unknown`, `Serving`, `NotServing`, `ServiceUnknown`).
- A service the server does not know gives `NotFound`: from `health_status`, and from `health_watch` when it opens the stream.
- A `GrpcServer` reports `NotServing` as soon as its shutdown future completes. Open `Watch` streams delay graceful shutdown, so drop the stream before waiting for the server task.

## Conformance checks

```rust
use rust_grpc_lib::testing::assert_policy_table_conforms;
use rust_grpc_lib::tonic::transport::Channel;

let channel = Channel::from_shared(format!("http://{addr}"))?.connect().await?;
assert_policy_table_conforms(channel, ExampleDbServer::<ExampleDbService>::new).await;
```

The second argument is the generated constructor, as in `add_service`. It is never called; it names the generated server and your implementation, so neither type is written twice.

The check calls every method in the macro-emitted table with an empty message: public methods must not be rejected without a token; every other method must answer `Unauthenticated` with no token and with an invalid one. It panics once, listing every mismatch, and also on an empty table. There is no method list to maintain.

The public check is weak: any status other than `Unauthenticated` or `PermissionDenied` passes, including `Unavailable`. A table with only public methods therefore passes even when the server is unreachable. Check that the server is up first, for example with `health_status(channel.clone(), "")`.

It does not check:

- **The success path or role denial.** Use a real client (above) or `assert_all_rpcs_require_role(channel, &paths, token_without_role)`.
- **Methods missing from the table.** Nothing at run time lists a tonic server's methods. The compiler catches most gaps, because every `fn` in an annotated block needs a policy. But two cases get no row and are caught only at run time, where they fail closed with `PermissionDenied "method not permitted"`:
  - A non-`fn` item in the block, such as a macro invocation that expands to a method. The macro skips it.
  - A trait method with a default body that the block doesn't override. The bundled tonic traits have no default bodies, so in practice only the first case applies.

  To catch both, use `assert_service_conforms(channel, ExampleDbServer::<ExampleDbService>::new, &["getEndPointOne", ..])` with an independent list of proto names. The list must match the table exactly.
- **Whether a public handler runs.** The table holds Rust names; tonic routes by exact proto name and answers `Unimplemented`. `assert_service_conforms` sends proto names.

`assert_all_rpcs_require_auth(channel, &paths)` and `call_rpc(channel, path, token)` take paths such as `/services.example.ExampleDB/getEndPointOne`; build them from the generated `SERVICE_NAME` const. Health and reflection paths are refused.

## Capturing spans

```rust
use rust_grpc_lib::testing::capture_spans;

#[tokio::test] // the default current-thread runtime
async fn emits_rpc_span() {
    let (spans, _guard) = capture_spans(); // keep the guard alive
    // ... make a call ...
    assert_eq!(spans.field("rpc", "grpc.status").as_deref(), Some("Ok"));
}
```

`spans.spans("rpc")` returns `SpanRecord`s (`name()`, `field(..)`, `has_field(..)`). The subscriber is thread-local, so a `multi_thread` runtime loses spans. Only spans are captured, not events. The fields are listed under [Tracing and logs](server.md#tracing-and-logs).
