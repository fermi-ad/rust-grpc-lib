//! Builder configuration, telemetry ownership, and live metric export.

use super::*;

use opentelemetry::Value;
use opentelemetry_sdk::metrics::{
    InMemoryMetricExporter, PeriodicReader,
    data::{AggregatedMetrics, MetricData},
};
use tokio::{sync::oneshot, time::timeout};
use tonic::codegen::http::uri::PathAndQuery;
use tonic::{Code, Status, client::Grpc, transport::Endpoint};
use tonic_health::pb::HealthCheckRequest;
use tonic_health::pb::health_check_response::ServingStatus as WireStatus;
use tonic_health::pb::health_server::Health;
use tonic_health::server::HealthService;
use tonic_prost::ProstCodec;

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

/// Read a service's status through the public `Health` service.
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
    assert!(builder.automatic_telemetry.get().is_none());

    let shared = in_memory().0;
    let config = TelemetryConfig::default()
        .endpoint("invalid")
        .sdk_disabled(true);
    let builder = builder.telemetry(&shared).telemetry_config(config.clone());
    drop(builder.meter()); // Works without a Tokio runtime.
    assert!(!builder.automatic_telemetry.get().unwrap().is_enabled());
    assert!(
        builder
            .telemetry
            .call_layer(&builder.automatic_telemetry, LabelIndex::new(), None)
            .is_some()
    );

    let builder = builder.without_telemetry();
    drop(builder.meter());
    assert!(builder.automatic_telemetry.get().is_none());
    assert!(
        builder
            .telemetry
            .call_layer(&builder.automatic_telemetry, LabelIndex::new(), None)
            .is_none()
    );

    let builder = builder.telemetry_config(config).telemetry(&shared);
    let resolved = builder
        .telemetry
        .resolve(&builder.automatic_telemetry)
        .unwrap();
    assert!(resolved.is_enabled());
    assert!(builder.automatic_telemetry.get().is_none());
}

#[tokio::test]
async fn automatic_telemetry_flushes_call_metrics_and_the_pre_serve_meter() {
    let (telemetry, exporter) = in_memory();
    let builder = GrpcServer::builder(test_validator(KID))
        .add_service(|stub| stub, Stub)
        .with_automatic_test_telemetry(telemetry.clone());
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
    let shared_builder = GrpcServer::builder(test_validator(KID)).telemetry(&shared);
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
        .telemetry(&shared)
        .without_telemetry()
        .on_call(move |_| {
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

#[tokio::test]
async fn transport_failure_is_preserved_after_automatic_shutdown() {
    let (telemetry, exporter) = in_memory();
    telemetry
        .meter()
        .u64_counter("test.failure")
        .build()
        .add(1, &[]);
    // Inject a tonic transport error at the return boundary: tonic retries TCP accept errors.
    let error = Endpoint::from_shared("http://[".to_owned()).unwrap_err();
    let expected = error.to_string();
    let actual = finish_serving(Err(error), Some(telemetry.clone()))
        .await
        .unwrap_err();
    assert_eq!(actual.to_string(), expected);
    assert!(!telemetry.is_enabled());
    assert!(!exporter.get_finished_metrics().unwrap().is_empty());
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
