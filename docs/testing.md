# Testing your service

How to test a service built with this crate: handler logic, access policy through a real server, health, spans and metrics. The helpers live in `rust_grpc_lib::testing`; the module rustdoc lists every one.

Enable them with `rust-grpc-lib` plus the `test-util` feature in `[dev-dependencies]` only: it ships a publicly known RSA key, and `test_validator` trusts anything signed with it. Add `tokio` (for `#[tokio::test]`) and nothing else; tonic types come from `rust_grpc_lib::tonic`, and `testing::health_status`, `health_watch` and `StreamExt` replace `tonic-health` and `futures-util`.

| To test | Use | Gate involved |
|---|---|---|
| Handler logic | [`authenticated_request`, `public_request`, `RequestContext::for_test`](#unit-test-a-handler) | No |
| Access policy and the real wire path | [`TestServer`, `test_token`](#test-a-service-end-to-end) | Yes |
| Every method enforces its policy | [`assert_policy_table_conforms`](#check-policies-conformance) | Yes |
| Health | [`health_status`, `health_watch`](#check-health) | n/a |
| `rpc` span fields | [`capture_spans`](#capture-spans) | n/a |
| Metrics | [`TestServer::with_metrics`, `InMemoryMetrics`](#assert-on-metrics) | n/a |

Examples use `ExampleDB` (`package services.example;`) implemented by `ExampleDbService`; substitute your own generated types.

## Unit-test a handler

After the macro rewrite, a handler takes a `rust_grpc_lib::tonic::Request<T>`. These helpers build one carrying what the gate would add, so a test can call the handler directly. The gate is **bypassed**: this tests handler logic, not access policy.

```rust
use rust_grpc_lib::testing::{authenticated_request, claims_with_roles, public_request};

let req = authenticated_request(IdList { id: vec![7, 8] }, claims_with_roles("alice", &["operator"]));
let reply = ExampleDbService.get_end_point_one(req).await.unwrap().into_inner();

// A #[public] method takes a request built by public_request.
let reply = MyService.my_public_method(public_request(Empty {})).await.unwrap().into_inner();
```

- A plain `Request::new`, or `public_request` passed to an authenticated handler, fails with `Internal "authentication is not configured"`.
- `claims_with_roles(sub, roles)` covers the common case; `KeycloakClaimsBuilder` builds custom claims (expiry, issuer, client roles).
- For logic that takes a context rather than a request, build one with `RequestContext::for_test(claims, msg)` or `PublicContext::for_test(msg)`. The context reports method `test.Service/Test` and token `test-token`. Contexts are read-only; chain `with_rpc(RpcMethod)`, `with_remote_addr(addr)`, `with_metadata(map)` or `with_extension(value)` to simulate more call details.

## Test a service end to end

`TestServer` runs your real service behind the real gate on a fresh IPv4 loopback port, and returns once health reports `Serving`:

```rust
use rust_grpc_lib::auth::ForwardedToken;
use rust_grpc_lib::testing::TestServer;
use rust_grpc_lib::tonic::Code;

#[tokio::test]
async fn operator_can_read() -> Result<(), Box<dyn std::error::Error>> {
    let server = TestServer::builder("test-kid")
        .add_service(ExampleDbServer::new, ExampleDbService)
        .start()
        .await;

    let token = server.token("alice", &["operator"]);
    let mut client = ExampleDbClient::authenticated(server.url(), ForwardedToken::new(token))?;
    let reply = client.get_end_point_one(IdList { id: vec![1, 2] }).await?.into_inner();
    // ... assert on reply ...

    // No token: a gated method answers Unauthenticated.
    let status = ExampleDbClient::public(server.url())?
        .get_end_point_one(IdList::default())
        .await
        .unwrap_err();
    assert_eq!(status.code(), Code::Unauthenticated);

    server.shutdown().await;
    Ok(())
}
```

- `TestServer::builder(kid)` accepts tokens minted by `server.token(sub, roles)` (valid one hour). Builder options: `.add_service(..)` (as in `GrpcServer`), `.timeout(d)`, `.with_metrics()`, `.shutdown_timeout(d)`. Getters: `url()`, `addr()`.
- It installs no signal handlers and no environment-configured exporter, and every test gets its own port, so tests run in parallel.
- Expected statuses: a garbage or expired token gives `Unauthenticated`; a valid token without the role gives `PermissionDenied "required role not present"`; `public` calls to `#[public]` methods succeed.
- `server.shutdown().await` drains gracefully and **panics** if the server fails or does not drain within five seconds (`.shutdown_timeout(d)` changes that). Drop open streams (including health `Watch`) first. Dropping the `TestServer` instead aborts the server without waiting.
- **Forwarding:** start the downstream and the gateway as two `TestServer`s built with the same `kid`, point the gateway at the downstream's `url()`, and call the gateway with a token. Each service enforces its own policy on the forwarded token.

### Custom setups

For a setup `TestServer` does not cover (a custom validator, `serve_with_shutdown`, your own listener), bind a listener yourself and use the `test-util` token helpers:

```rust
use rust_grpc_lib::server::GrpcServer;
use rust_grpc_lib::testing::{expired_test_token, test_token, test_validator};

let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
let addr = listener.local_addr()?;
let (stop, rx) = tokio::sync::oneshot::channel::<()>(); // dropping `stop` also shuts the server down
let server = tokio::spawn(
    GrpcServer::builder(test_validator("kid"))
        .add_service(ExampleDbServer::new, ExampleDbService)
        .serve_on_listener(listener, async { let _ = rx.await; }),
);
let token = test_token("kid", "alice", &["operator"]);
let expired = expired_test_token("kid", "alice", &["operator"]); // expired two minutes ago
// ... call http://{addr} ...
let _ = stop.send(());
server.await??; // Ok(()) after a graceful drain
```

`test_validator(kid)` rejects tokens minted with another `kid`. The listener is bound before the task starts, so early connections queue and no startup delay is needed.

## Check policies (conformance)

```rust
use rust_grpc_lib::testing::assert_policy_table_conforms;
use rust_grpc_lib::tonic::transport::Channel;

let channel = Channel::from_shared(server.url().to_owned())?.connect().await?;
assert_policy_table_conforms(channel, ExampleDbServer::<ExampleDbService>::new).await;
```

The second argument is the generated constructor, as in `add_service`; it is never called, only names the generated server and your implementation. The check reads your macro-emitted table and calls every method with an empty message:

- `#[public]` methods must not answer `Unauthenticated` or `PermissionDenied` without a token.
- Every other method must answer `Unauthenticated` with no token and with an invalid one.

It panics once, listing every mismatch (`service conformance failed for N reason(s)`), and also panics on an empty table. There is no method list to maintain.

It does **not** check:

- **The success path or role denial.** Use a real client (above) or `assert_all_rpcs_require_role(channel, &paths, token_without_role)`.
- **Methods missing from the table.** The compiler normally closes this gap. For an independent list use `assert_service_conforms(channel, ExampleDbServer::<ExampleDbService>::new, &["getEndPointOne", "getEndPointTwo"])`, which must match the table exactly and sends proto names, so public handlers are actually reached.
- **That a public handler runs.** `assert_policy_table_conforms` sends Rust names; tonic routes by exact proto name and answers `Unimplemented`.

Lower-level: `assert_all_rpcs_require_auth(channel, &paths)` and `call_rpc(channel, path, token)` take paths like `/services.example.ExampleDB/getEndPointOne`; build them from the generated `SERVICE_NAME` (`format!("/{SERVICE_NAME}/getEndPointOne")`). Health and reflection paths are refused.

## Check health

`GrpcServer` serves `grpc.health.v1.Health` without a token, and two helpers query it:

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

- `health_status` returns `Result<ServingStatus, Status>`; `health_watch` returns `Result<impl Stream<Item = Result<ServingStatus, Status>>, Status>`. `StreamExt` is re-exported so you need no `futures-util`.
- `ServingStatus` is the `grpc.health.v1` enum: `Unknown`, `Serving`, `NotServing`, `ServiceUnknown`.
- A service the server does not know gives `NotFound`, from `health_status` and from `health_watch` when it opens the stream.
- The server reports `NotServing` as soon as shutdown begins. Open `Watch` streams delay graceful shutdown, so drop the stream before waiting for the server to finish.

## Capture spans

```rust
use rust_grpc_lib::testing::capture_spans;

#[tokio::test] // the default current-thread runtime
async fn emits_rpc_span() {
    let (spans, _guard) = capture_spans(); // keep the guard alive
    // ... start a server and make a call ...
    assert_eq!(spans.field("rpc", "grpc.status").as_deref(), Some("Ok"));
}
```

`spans.spans("rpc")` returns `SpanRecord`s (`name()`, `field(..)`, `has_field(..)`); `spans.field(span, field)` reads the first span of that name. Values are strings (`Ok`, not `0`). The subscriber is thread-local, so a `multi_thread` runtime loses spans. Only spans are captured, not events. Field meanings are in [Tracing and logs](server.md#tracing-and-logs).

## Assert on metrics

Use `TestServer::builder(kid).with_metrics()`; it records into an isolated in-memory backend regardless of environment settings. `server.metrics()` panics unless `.with_metrics()` was selected.

```rust
use rust_grpc_lib::{
    testing::{TestServer, health_status},
    tonic::{Code, transport::Channel},
};

#[tokio::test]
async fn records_health_calls() {
    let server = TestServer::builder("test-kid").with_metrics().start().await;
    let channel = Channel::from_shared(server.url().to_owned()).unwrap().connect().await.unwrap();
    let error = health_status(channel, "example.unregistered.Service").await.unwrap_err();
    assert_eq!(error.code(), Code::NotFound);
    // The harness's own startup health checks (status Ok) are not counted here.
    assert_eq!(server.metrics().wait_for_call_count("grpc.health.v1.Health/Check", Code::NotFound).await, 1);
    server.shutdown().await;
}
```

| Helper | Use |
|---|---|
| `wait_for_call_count(method, code).await` | Waits up to five seconds for the first matching completion and returns that series' cumulative count; panics on timeout. `method` is the [exact metric label](server.md#built-in-metrics) (`package.Service/Method`, or `_OTHER`) |
| `wait_until(\|metrics\| .., timeout).await` | Waits for any condition; panics on timeout, showing the recorded series |
| `histograms(name)` | `HistogramSnapshot { unit, attributes, count, bounds, bucket_counts }` per point |
| `counters(name)` | `CounterSnapshot { attributes, value }`; includes up-down counters |

Account for startup health checks when asserting totals. A response can reach the client before recording finishes, and a cancellation is recorded only once the server sees the drop, so wait instead of reading immediately. Snapshot attribute values are always strings, even for numeric ones (`rpc.response.status_code` is `"5"`, not `5`).

**Existing server fixture?** Create `InMemoryMetrics::new()` and give its builder `.with_caller_owned_telemetry(&metrics.telemetry())`. There is no collector and no runtime probe. You own the provider: after serving ends, call `metrics.telemetry().shutdown().await` (see [Provider ownership](server.md#provider-ownership)); snapshots stay readable afterward.
