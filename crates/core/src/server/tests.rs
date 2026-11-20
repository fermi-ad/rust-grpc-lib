//! Builder configuration, telemetry ownership, and live metric export.

use super::*;

use opentelemetry::Value;
use opentelemetry_sdk::metrics::{
    InMemoryMetricExporter, PeriodicReader,
    data::{AggregatedMetrics, MetricData},
};
use tokio::{sync::oneshot, time::timeout};
use tonic::codegen::http::uri::PathAndQuery;
use tonic::{Code, Status, Streaming, client::Grpc, transport::Endpoint};
use tonic_health::pb::HealthCheckRequest;
use tonic_health::pb::HealthCheckResponse;
use tonic_health::pb::health_check_response::ServingStatus as WireStatus;
use tonic_health::pb::health_client::HealthClient;
use tonic_health::pb::health_server::Health;
use tonic_health::server::HealthService;
use tonic_prost::ProstCodec;
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::{Layer, layer::Context, prelude::*};

use crate::auth::{Access, MethodPolicy};
use crate::testing::{health_status, test_token, test_validator};

const KID: &str = "server-telemetry";

#[derive(Clone)]
struct Stub;

impl NamedService for Stub {
    const NAME: &'static str = "test.Service";
}

impl WireMethods for Stub {
    const WIRE_METHODS: &'static [&'static str] = &["test.Service/CheckToken"];
}

impl ServicePolicy<Stub> for Stub {
    const POLICIES: &'static [MethodPolicy] =
        &[MethodPolicy::new("check_token", Access::Authenticated)];
}

impl Service<Request<Body>> for Stub {
    type Response = Response<Body>;
    type Error = Infallible;
    type Future = tonic::codegen::BoxFuture<Self::Response, Self::Error>;

    fn poll_ready(
        &mut self,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: Request<Body>) -> Self::Future {
        struct CheckToken;

        impl tonic::server::UnaryService<()> for CheckToken {
            type Response = ();
            type Future = std::future::Ready<Result<tonic::Response<()>, Status>>;

            fn call(&mut self, _: tonic::Request<()>) -> Self::Future {
                std::future::ready(Ok(tonic::Response::new(())))
            }
        }

        Box::pin(async move {
            Ok(tonic::server::Grpc::new(ProstCodec::default())
                .unary(CheckToken, request)
                .await)
        })
    }
}

async fn status_of(reporter: &HealthReporter, name: &str) -> WireStatus {
    HealthService::from_health_reporter(reporter.clone())
        .check(tonic::Request::new(HealthCheckRequest {
            service: name.to_string(),
        }))
        .await
        .unwrap()
        .into_inner()
        .status()
}

/// Server-owned telemetry that is already started, so tests can read its exports.
fn server_owned(telemetry: Telemetry) -> TelemetrySource {
    TelemetrySource::ServerOwned(None, OnceLock::from(telemetry))
}

#[tokio::test]
async fn without_signal_handling_keeps_serving() {
    let builder = GrpcServer::builder(test_validator(KID))
        .without_telemetry()
        .without_signal_handling();
    let addr = "127.0.0.1:0".parse().unwrap();
    let result = timeout(Duration::from_millis(100), builder.serve(addr)).await;
    assert!(result.is_err(), "serve unexpectedly returned: {result:?}");
}

#[tokio::test]
async fn signal_opt_out_still_returns_bind_errors() {
    let occupied = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let err = GrpcServer::builder(test_validator(KID))
        .without_signal_handling()
        .serve(occupied.local_addr().unwrap())
        .await
        .unwrap_err();
    assert!(matches!(err, ServeError::Bind(_)), "{err:?}");
}

#[tokio::test(start_paused = true)]
async fn shutdown_without_connections_does_not_wait_for_grace() {
    let builder = GrpcServer::builder(test_validator(KID)).without_telemetry();
    assert_eq!(builder.shutdown_grace, Duration::from_secs(30));
    let start = tokio::time::Instant::now();
    builder
        .serve_with_shutdown("127.0.0.1:0".parse().unwrap(), async {})
        .await
        .unwrap();
    assert_eq!(start.elapsed(), Duration::ZERO);
}

#[tokio::test]
async fn shutdown_grace_starts_at_signal_and_cancels_streams_before_flushing() {
    let grace = Duration::from_millis(200);
    let (telemetry, exporter) = in_memory();
    let (summaries, mut calls) = tokio::sync::mpsc::unbounded_channel();
    let mut builder = GrpcServer::builder(test_validator(KID))
        .shutdown_grace(grace)
        .on_call_complete(move |summary| summaries.send(*summary).unwrap());
    builder.telemetry = server_owned(telemetry.clone());
    let (warnings, mut events) = tokio::sync::mpsc::unbounded_channel();
    // Keep two dispatches registered so tracing combines their callsite
    // interests instead of caching just the first polling thread's default.
    // Other tests can reach this same warning callsite without a subscriber.
    let _no_events = tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default());
    // The server task runs on this test's current thread.
    let _subscriber = tracing::subscriber::set_default(
        tracing_subscriber::registry().with(GraceWarnings(warnings)),
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (stop, signal) = oneshot::channel();
    let task = tokio::spawn(builder.serve_on_listener(listener, async { signal.await.unwrap() }));

    // Use distinct connections so expiry has to wake every socket, not just
    // the last one to register an I/O waker.
    let mut first = open_watch(addr).await;
    tokio::time::sleep(grace).await;
    let mut second = open_watch(addr).await;
    assert!(!task.is_finished(), "grace must not limit normal serving");
    assert!(events.try_recv().is_err());
    assert_eq!(telemetry.runtime_probe_server_count(), 1);
    assert!(exporter.get_finished_metrics().unwrap().is_empty());

    let start = tokio::time::Instant::now();
    stop.send(()).unwrap();
    assert_eq!(watch_status(&mut first).await, WireStatus::NotServing);
    assert_eq!(telemetry.runtime_probe_server_count(), 0);
    timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        start.elapsed() >= grace,
        "connections closed before the deadline"
    );
    events.try_recv().expect("grace expiry must log a warning");
    assert!(events.try_recv().is_err());

    assert_watch_closed(&mut first).await;
    assert_watch_closed(&mut second).await;
    for _ in 0..2 {
        let call = calls.try_recv().unwrap();
        assert_eq!(call.method, Some("grpc.health.v1.Health/Watch"));
        assert_eq!(call.status, Code::Cancelled);
    }
    assert!(calls.try_recv().is_err());
    assert!(!telemetry.is_enabled());

    let exports = exporter.get_finished_metrics().unwrap();
    let metric = |name| {
        exports[0]
            .scope_metrics()
            .flat_map(|scope| scope.metrics())
            .find(|metric| metric.name() == name)
            .unwrap()
            .data()
    };
    let AggregatedMetrics::F64(MetricData::Histogram(duration)) =
        metric("rpc.server.call.duration")
    else {
        panic!("expected the duration histogram");
    };
    let point = duration.data_points().next().unwrap();
    assert_eq!(point.count(), 2);
    assert!(point.attributes().any(|attr| {
        attr.key.as_str() == "rpc.response.status_code"
            && attr.value == Value::from(Code::Cancelled as i64)
    }));
    let AggregatedMetrics::I64(MetricData::Sum(active)) = metric("rpc.server.active_requests")
    else {
        panic!("expected the active requests counter");
    };
    assert_eq!(active.data_points().next().unwrap().value(), 0);
}

#[tokio::test]
async fn zero_grace_closes_a_live_stream_on_the_address_bound_path() {
    let builder = GrpcServer::builder(test_validator(KID))
        .without_telemetry()
        .shutdown_grace(Duration::ZERO);
    let health = builder.health.clone();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let (ready, bound) = oneshot::channel();
    let (stop, signal) = oneshot::channel();
    let task = tokio::spawn(builder.serve_with_shutdown(addr, async {
        ready.send(()).unwrap();
        signal.await.unwrap();
    }));
    timeout(Duration::from_secs(5), bound)
        .await
        .unwrap()
        .unwrap();
    let mut watch = open_watch(addr).await;
    stop.send(()).unwrap();
    timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(status_of(&health, "").await, WireStatus::NotServing);
    assert_watch_closed(&mut watch).await;
}

#[tokio::test]
async fn cancelling_serve_closes_live_connections() {
    let builder = GrpcServer::builder(test_validator(KID)).without_telemetry();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(builder.serve_on_listener(listener, pending()));
    let mut watch = open_watch(addr).await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_watch_closed(&mut watch).await;
}

async fn open_watch(addr: SocketAddr) -> Streaming<HealthCheckResponse> {
    timeout(Duration::from_secs(5), async {
        let channel = Endpoint::from_shared(format!("http://{addr}"))
            .unwrap()
            .connect()
            .await
            .unwrap();
        let mut watch = HealthClient::new(channel)
            .watch(HealthCheckRequest::default())
            .await
            .unwrap()
            .into_inner();
        assert_eq!(watch_status(&mut watch).await, WireStatus::Serving);
        watch
    })
    .await
    .expect("health Watch must open")
}

async fn watch_status(watch: &mut Streaming<HealthCheckResponse>) -> WireStatus {
    timeout(Duration::from_secs(5), watch.message())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .status()
}

async fn assert_watch_closed(watch: &mut Streaming<HealthCheckResponse>) {
    timeout(Duration::from_secs(5), async {
        // NotServing may already be buffered, but the stream must then end.
        while let Ok(Some(_)) = watch.message().await {}
    })
    .await
    .expect("the client's open Watch must end without being dropped by the client");
}

struct GraceWarnings(tokio::sync::mpsc::UnboundedSender<()>);

impl<S: Subscriber> Layer<S> for GraceWarnings {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        let metadata = event.metadata();
        if metadata.level() == &Level::WARN
            && metadata.target() == module_path!().strip_suffix("::tests").unwrap()
            && metadata.fields().field("grace").is_some()
        {
            self.0.send(()).unwrap();
        }
    }
}

#[tokio::test]
async fn set_status_updates_the_overall_service_and_every_listed_service() {
    let (reporter, _service) = health_reporter();

    set_status(&reporter, &["a.A", "b.B"], ServingStatus::NotServing).await;

    for name in ["", "a.A", "b.B"] {
        assert_eq!(
            status_of(&reporter, name).await,
            WireStatus::NotServing,
            "service {name:?}"
        );
    }
}

#[test]
fn telemetry_is_lazy_and_the_last_selection_replaces_a_cached_meter() {
    let builder = GrpcServer::builder(test_validator(KID));
    assert!(
        matches!(&builder.telemetry, TelemetrySource::ServerOwned(None, cache) if cache.get().is_none())
    );

    let shared = in_memory().0;
    let config = TelemetryConfig::default()
        .endpoint("invalid")
        .sdk_disabled(true);
    let builder = builder
        .with_caller_owned_telemetry(&shared)
        .telemetry_config(config.clone());
    drop(builder.meter()); // Works without a Tokio runtime.
    assert!(
        matches!(&builder.telemetry, TelemetrySource::ServerOwned(Some(_), cache)
        if cache.get().is_some_and(|telemetry| !telemetry.is_enabled()))
    );

    let builder = builder.without_telemetry();
    drop(builder.meter());
    assert!(builder.telemetry.resolve().is_none());

    let builder = builder
        .telemetry_config(config)
        .with_caller_owned_telemetry(&shared);
    assert!(
        matches!(builder.telemetry, TelemetrySource::CallerOwned(telemetry) if telemetry.is_enabled())
    );
}

#[tokio::test]
async fn automatic_telemetry_flushes_call_metrics_and_the_pre_serve_meter() {
    let (telemetry, exporter) = in_memory();
    let mut builder = GrpcServer::builder(test_validator(KID)).add_service(|stub| stub, Stub);
    builder.telemetry = server_owned(telemetry.clone());
    builder
        .meter()
        .u64_counter("test.before_serve")
        .build()
        .add(7, &[]);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint =
        Endpoint::from_shared(format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let (stop, signal) = oneshot::channel();
    let task = tokio::spawn(builder.serve_on_listener(listener, async { signal.await.unwrap() }));

    let channel = endpoint.connect().await.unwrap();
    let mut client = Grpc::new(channel.clone());
    let mut request = tonic::Request::new(());
    request.metadata_mut().insert(
        "authorization",
        format!("Bearer {}", test_token(KID, "tester", &[]))
            .parse()
            .unwrap(),
    );
    let method = "/test.Service/CheckToken";
    grpc_call(&mut client, request, method).await.unwrap();
    assert_eq!(
        grpc_call(&mut client, tonic::Request::new(()), method)
            .await
            .unwrap_err()
            .code(),
        Code::Unauthenticated
    );
    assert_eq!(
        health_status(channel, "").await.unwrap(),
        WireStatus::Serving
    );
    assert_eq!(
        grpc_call(
            &mut client,
            tonic::Request::new(()),
            "/unknown.Service/Unknown"
        )
        .await
        .unwrap_err()
        .code(),
        Code::Unimplemented
    );

    assert!(telemetry.is_enabled());
    stop.send(()).unwrap();
    timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(!telemetry.is_enabled());

    let exports = exporter.get_finished_metrics().unwrap();
    let metric = |name| {
        exports[0]
            .scope_metrics()
            .flat_map(|scope| scope.metrics())
            .find(|metric| metric.name() == name)
            .unwrap()
            .data()
    };
    let AggregatedMetrics::U64(MetricData::Sum(custom)) = metric("test.before_serve") else {
        panic!("expected the custom counter");
    };
    assert_eq!(custom.data_points().next().unwrap().value(), 7);
    let AggregatedMetrics::F64(MetricData::Histogram(duration)) =
        metric("rpc.server.call.duration")
    else {
        panic!("expected the call duration histogram");
    };
    assert_eq!(duration.data_points().count(), 4);
    for (method, status) in [
        ("test.Service/CheckToken", Code::Ok),
        ("test.Service/CheckToken", Code::Unauthenticated),
        ("grpc.health.v1.Health/Check", Code::Ok),
        ("_OTHER", Code::Unimplemented),
    ] {
        let point = duration
            .data_points()
            .find(|point| {
                let has = |key, value| {
                    point
                        .attributes()
                        .any(|attr| attr.key.as_str() == key && attr.value == value)
                };
                has("rpc.method", Value::from(method))
                    && has("rpc.response.status_code", Value::from(status as i64))
            })
            .unwrap_or_else(|| panic!("missing {method} {status:?}"));
        assert_eq!(point.count(), 1);
    }
}

#[tokio::test]
async fn server_leaves_shared_telemetry_running_and_off_meter_exports_nothing() {
    let (shared, exporter) = in_memory();
    let shared_builder =
        GrpcServer::builder(test_validator(KID)).with_caller_owned_telemetry(&shared);
    shared_builder
        .meter()
        .u64_counter("test.shared")
        .build()
        .add(1, &[]);
    shared_builder
        .serve_with_shutdown("127.0.0.1:0".parse().unwrap(), async {})
        .await
        .unwrap();
    assert!(shared.is_enabled());
    assert!(exporter.get_finished_metrics().unwrap().is_empty());

    let (observed, count) = tokio::sync::mpsc::unbounded_channel();
    let off = GrpcServer::builder(test_validator(KID))
        .with_caller_owned_telemetry(&shared)
        .without_telemetry()
        .on_call_complete(move |_| {
            let _ = observed.send(());
        });
    off.meter().u64_counter("test.off").build().add(1, &[]);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint =
        Endpoint::from_shared(format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let (stop, signal) = oneshot::channel();
    let task = tokio::spawn(off.serve_on_listener(listener, async { signal.await.unwrap() }));
    assert_eq!(
        health_status(endpoint.connect().await.unwrap(), "")
            .await
            .unwrap(),
        WireStatus::Serving
    );
    stop.send(()).unwrap();
    timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let mut count = count;
    assert!(
        count.try_recv().is_err(),
        "opt-out must suppress the observer"
    );
    assert!(shared.is_enabled());

    shared.shutdown().await;
    let exports = exporter.get_finished_metrics().unwrap();
    assert!(!exports.is_empty());
    for export in &exports {
        let names: Vec<_> = export
            .scope_metrics()
            .flat_map(|scope| scope.metrics())
            .map(|metric| metric.name())
            .collect();
        assert_eq!(names, ["test.shared"]);
    }
}

#[tokio::test(start_paused = true)]
async fn shared_runtime_probe_tracks_servers_until_shutdown_or_cancellation() {
    let (shared, _) = in_memory();
    let builder = || GrpcServer::builder(test_validator(KID)).with_caller_owned_telemetry(&shared);
    drop(builder().meter());
    assert_eq!(shared.runtime_probe_server_count(), 0);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint =
        Endpoint::from_shared(format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let (stop, signal) = oneshot::channel();
    let (started, ready) = oneshot::channel();
    let first = tokio::spawn(builder().serve_on_listener(listener, async {
        started.send(()).unwrap();
        signal.await.unwrap();
    }));
    ready.await.unwrap();
    assert_eq!(shared.runtime_probe_server_count(), 1);

    let (started, ready) = oneshot::channel();
    let second = tokio::spawn(builder().serve_with_shutdown(
        "127.0.0.1:0".parse().unwrap(),
        async {
            started.send(()).unwrap();
            pending::<()>().await;
        },
    ));
    ready.await.unwrap();
    assert_eq!(shared.runtime_probe_server_count(), 2);

    let (started, ready) = oneshot::channel();
    let off = tokio::spawn(builder().without_telemetry().serve_with_shutdown(
        "127.0.0.1:0".parse().unwrap(),
        async {
            started.send(()).unwrap();
            pending::<()>().await;
        },
    ));
    ready.await.unwrap();
    assert_eq!(shared.runtime_probe_server_count(), 2);

    // Hold a streaming RPC open so graceful drain cannot finish yet.
    let mut watch =
        tonic_health::pb::health_client::HealthClient::new(endpoint.connect().await.unwrap())
            .watch(HealthCheckRequest::default())
            .await
            .unwrap()
            .into_inner();
    assert_eq!(
        watch.message().await.unwrap().unwrap().status(),
        WireStatus::Serving
    );
    second.abort();
    assert!(second.await.unwrap_err().is_cancelled());
    assert_eq!(shared.runtime_probe_server_count(), 1);

    stop.send(()).unwrap();
    assert_eq!(
        watch.message().await.unwrap().unwrap().status(),
        WireStatus::NotServing
    );
    assert!(!first.is_finished());
    assert_eq!(shared.runtime_probe_server_count(), 0);
    drop(watch);
    first.await.unwrap().unwrap();
    assert!(shared.is_enabled());
    off.abort();
    assert!(off.await.unwrap_err().is_cancelled());
}

fn in_memory() -> (Telemetry, InMemoryMetricExporter) {
    let exporter = InMemoryMetricExporter::default();
    let reader = PeriodicReader::builder(exporter.clone())
        .with_interval(Duration::from_secs(600))
        .build();
    (
        Telemetry::from_reader(reader, TelemetryConfig::default()),
        exporter,
    )
}

async fn grpc_call(
    client: &mut Grpc<tonic::transport::Channel>,
    request: tonic::Request<()>,
    path: &'static str,
) -> Result<tonic::Response<()>, Status> {
    client.ready().await.unwrap();
    client
        .unary(
            request,
            PathAndQuery::from_static(path),
            ProstCodec::default(),
        )
        .await
}
