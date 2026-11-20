//! Exercise the recorder through the real call layer with fake services and bodies.

use std::{
    collections::VecDeque,
    future::{pending, poll_fn, ready},
    pin::Pin,
    sync::Mutex,
    task::{Context, Poll},
    time::Duration,
};

use http_body::{Body, Frame};
use opentelemetry_sdk::metrics::{
    InMemoryMetricExporter, PeriodicReader,
    data::{AggregatedMetrics, HistogramDataPoint, Metric, MetricData, ResourceMetrics},
    reader::MetricReader,
};
use tonic::codegen::{
    Bytes,
    http::{HeaderMap, Request, Response},
};
use tower::{Layer, Service, service_fn};

use super::{super::tests::capture_logs, *};
use crate::{
    auth::{Access, MethodPolicy, PolicyTable, ServicePolicy},
    telemetry::{TelemetryConfig, label::MethodLabelIndex, layer::CallLayer},
};

const METHOD: &str = "pkg.Service/GetValue";
const PATH: &str = "/pkg.Service/GetValue";
const BOUNDS: [f64; 2] = [0.001, 0.1];

struct TestPolicy;

impl ServicePolicy<()> for TestPolicy {
    const POLICIES: &'static [MethodPolicy] = &[MethodPolicy::new("get_value", Access::Public)];
}

#[derive(Default)]
struct TestBody(VecDeque<Frame<Bytes>>);

impl Body for TestBody {
    type Data = Bytes;
    type Error = std::convert::Infallible;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        Poll::Ready(self.0.pop_front().map(Ok))
    }
}

#[tokio::test]
async fn duration_counts_every_status_health_and_unlisted_paths() {
    let (telemetry, reader) = telemetry();
    let layer = layer(&telemetry, None);
    // These are fake trailers-only responses, including the same status codes
    // that the access gate can return before dispatch.
    let mut service = layer.layer(service_fn(|request: Request<&'static str>| {
        ready(
            Response::builder()
                .header("grpc-status", *request.body())
                .body(TestBody::default()),
        )
    }));
    let mut expected = Vec::new();
    for (code, status) in [
        "0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15", "16",
    ]
    .into_iter()
    .enumerate()
    {
        let response = service.call(request(PATH, status)).await.unwrap();
        assert_eq!(response.headers()["grpc-status"], status);
        let name = (code != 0).then(|| format!("{:?}", Code::from_bytes(status.as_bytes())));
        expected.push((METHOD, code as i64, name, 1));
    }
    for (path, status) in [
        ("/grpc.health.v1.Health/Check", "0"),
        ("/grpc.health.v1.Health/Watch", "0"),
        ("/pkg.Service/getValue", "12"),
        ("/arbitrary.Service/Unlisted", "12"),
    ] {
        drop(service.call(request(path, status)).await.unwrap());
    }
    expected.extend([
        ("grpc.health.v1.Health/Check", 0, None, 1),
        ("grpc.health.v1.Health/Watch", 0, None, 1),
        ("_OTHER", 12, Some("Unimplemented".into()), 2),
    ]);

    let metrics = collect(&reader);
    let points = duration_points(&metrics);
    assert_eq!(points.len(), expected.len());
    assert_eq!(points.iter().map(|point| point.count()).sum::<u64>(), 21);
    assert!(points.iter().map(|point| point.sum()).sum::<f64>() > 0.0);
    for (method, code, error, count) in expected {
        let mut attrs = method_attrs(method);
        attrs.push(KeyValue::new("rpc.response.status_code", code));
        if let Some(error) = error {
            attrs.push(KeyValue::new("error.type", error));
        }
        let point = points
            .iter()
            .find(|point| has_attrs(point.attributes(), &attrs))
            .unwrap();
        assert_eq!(point.count(), count);
        assert_eq!(point.bounds().collect::<Vec<_>>(), BOUNDS);
    }
    assert_active(
        &metrics,
        &[
            (METHOD, 0),
            ("grpc.health.v1.Health/Check", 0),
            ("grpc.health.v1.Health/Watch", 0),
            ("_OTHER", 0),
        ],
    );
    telemetry.shutdown().await;
}

#[tokio::test]
async fn active_tracks_overlapping_calls_and_observer_matches_terminal_metrics() {
    let (telemetry, reader) = telemetry();
    let observed = Arc::new(Mutex::new(Vec::new()));
    let summaries = Arc::clone(&observed);
    let layer = layer(
        &telemetry,
        Some(Arc::new(move |summary| {
            summaries.lock().unwrap().push(*summary)
        })),
    );

    let mut waiting = layer.layer(service_fn(|_: Request<()>| {
        pending::<Result<Response<TestBody>, ()>>()
    }));
    let mut streaming = layer.layer(service_fn(|_: Request<()>| {
        let mut trailers = HeaderMap::new();
        trailers.insert("grpc-status", "0".parse().unwrap());
        ready(Ok::<_, ()>(Response::new(TestBody(
            [
                Frame::data(Bytes::from_static(b"data")),
                Frame::trailers(trailers),
            ]
            .into(),
        ))))
    }));
    let future = waiting.call(request(PATH, ()));
    let mut cancelled = streaming.call(request(PATH, ())).await.unwrap().into_body();
    let mut completed = streaming.call(request(PATH, ())).await.unwrap().into_body();
    assert!(next_frame(&mut cancelled).await.is_data());
    assert!(next_frame(&mut completed).await.is_data());
    let metrics = collect(&reader);
    assert_active(&metrics, &[(METHOD, 3)]);
    assert!(metric(&metrics, "rpc.server.call.duration").is_none());
    assert!(observed.lock().unwrap().is_empty());

    drop(future);
    assert_active(&collect(&reader), &[(METHOD, 2)]);
    drop(cancelled);
    assert_active(&collect(&reader), &[(METHOD, 1)]);
    assert_eq!(
        next_frame(&mut completed).await.trailers_ref().unwrap()["grpc-status"],
        "0"
    );
    drop(completed);

    let metrics = collect(&reader);
    assert_active(&metrics, &[(METHOD, 0)]);
    let points = duration_points(&metrics);
    assert_eq!(points.len(), 2);
    let summaries = std::mem::take(&mut *observed.lock().unwrap());
    assert_eq!(
        summaries
            .iter()
            .map(|s| (s.method, s.status))
            .collect::<Vec<_>>(),
        [
            (Some(METHOD), Code::Cancelled),
            (Some(METHOD), Code::Cancelled),
            (Some(METHOD), Code::Ok),
        ]
    );
    for (code, name, count) in [(Code::Cancelled, Some("Cancelled"), 2), (Code::Ok, None, 1)] {
        let mut attrs = method_attrs(METHOD);
        attrs.push(KeyValue::new("rpc.response.status_code", code as i64));
        if let Some(name) = name {
            attrs.push(KeyValue::new("error.type", name));
        }
        let point = points
            .iter()
            .find(|point| has_attrs(point.attributes(), &attrs))
            .unwrap();
        assert_eq!(point.count(), count);
        let duration = summaries
            .iter()
            .filter(|summary| summary.status == code)
            .map(|summary| summary.duration.as_secs_f64())
            .sum::<f64>();
        assert!(duration > 0.0);
        assert_eq!(point.sum(), duration);
    }
    telemetry.shutdown().await;
}

#[tokio::test]
async fn observer_panic_leaves_response_and_metrics_intact() {
    let (telemetry, reader) = telemetry();
    let (logs, _guard) = capture_logs();
    let observed = Arc::new(Mutex::new(Vec::new()));
    let summaries = Arc::clone(&observed);
    let layer = layer(
        &telemetry,
        Some(Arc::new(move |summary| {
            summaries.lock().unwrap().push(*summary);
            panic!("observer failure");
        })),
    );
    let mut service = layer.layer(service_fn(|_: Request<()>| {
        ready(
            Response::builder()
                .header("grpc-status", "5")
                .body(TestBody::default()),
        )
    }));
    let response = service
        .call(request("/unlisted.Service/Call", ()))
        .await
        .unwrap();
    assert_eq!(response.headers()["grpc-status"], "5");
    drop(response);
    let summaries = std::mem::take(&mut *observed.lock().unwrap());
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].method, None);
    assert_eq!(summaries[0].status, Code::NotFound);
    let text = logs.text();
    assert!(text.contains("ERROR"));
    assert!(text.contains("rust_grpc_lib::telemetry"));
    assert!(text.contains("gRPC call observer panicked"));

    let metrics = collect(&reader);
    assert_active(&metrics, &[("_OTHER", 0)]);
    let points = duration_points(&metrics);
    assert_eq!(points.len(), 1);
    assert_eq!(points[0].count(), 1);
    assert_eq!(points[0].sum(), summaries[0].duration.as_secs_f64());
    let mut attrs = method_attrs("_OTHER");
    attrs.extend([
        KeyValue::new("rpc.response.status_code", 5_i64),
        KeyValue::new("error.type", "NotFound"),
    ]);
    assert!(has_attrs(points[0].attributes(), &attrs));
    telemetry.shutdown().await;
}

#[tokio::test]
async fn noop_meter_still_notifies_observer() {
    let observed = Arc::new(Mutex::new(Vec::new()));
    let summaries = Arc::clone(&observed);
    let layer = layer(
        &Telemetry::start(TelemetryConfig::default()),
        Some(Arc::new(move |summary| {
            summaries.lock().unwrap().push(*summary)
        })),
    );
    let mut service = layer.layer(service_fn(|_: Request<()>| {
        ready(Err::<Response<TestBody>, _>("inner error"))
    }));
    assert_eq!(
        service.call(request(PATH, ())).await.err(),
        Some("inner error")
    );
    let observed = observed.lock().unwrap();
    assert_eq!(observed.len(), 1);
    assert_eq!(observed[0].method, Some(METHOD));
    assert_eq!(observed[0].status, Code::Unknown);
}

fn telemetry() -> (Telemetry, impl MetricReader) {
    let reader = PeriodicReader::builder(InMemoryMetricExporter::default())
        .with_interval(Duration::from_secs(600))
        .build();
    (
        Telemetry::from_reader(
            reader.clone(),
            TelemetryConfig::default().duration_bucket_bounds_seconds(BOUNDS),
        ),
        reader,
    )
}

fn layer(telemetry: &Telemetry, on_call: Option<CallCompletionObserver>) -> CallLayer {
    let mut labels = MethodLabelIndex::new();
    labels.register(
        "pkg.Service",
        &[METHOD],
        PolicyTable::of::<(), TestPolicy>(),
    );
    CallLayer::new(telemetry, labels, on_call, watch::channel(()).1)
}

fn request<T>(path: &str, body: T) -> Request<T> {
    Request::builder().uri(path).body(body).unwrap()
}

async fn next_frame(
    body: &mut (impl Body<Data = Bytes, Error = std::convert::Infallible> + Unpin),
) -> Frame<Bytes> {
    poll_fn(|cx| Pin::new(&mut *body).poll_frame(cx))
        .await
        .unwrap()
        .unwrap()
}

fn collect(reader: &impl MetricReader) -> ResourceMetrics {
    let mut metrics = ResourceMetrics::default();
    reader.collect(&mut metrics).unwrap();
    metrics
}

fn metric<'a>(metrics: &'a ResourceMetrics, name: &str) -> Option<&'a Metric> {
    metrics
        .scope_metrics()
        .flat_map(|scope| scope.metrics())
        .find(|metric| metric.name() == name)
}

fn duration_points(metrics: &ResourceMetrics) -> Vec<&HistogramDataPoint<f64>> {
    let metric = metric(metrics, "rpc.server.call.duration").unwrap();
    assert_eq!(metric.unit(), "s");
    let AggregatedMetrics::F64(MetricData::Histogram(histogram)) = metric.data() else {
        panic!("expected an f64 histogram");
    };
    histogram.data_points().collect()
}

fn assert_active(metrics: &ResourceMetrics, expected: &[(&'static str, i64)]) {
    let metric = metric(metrics, "rpc.server.active_requests").unwrap();
    let AggregatedMetrics::I64(MetricData::Sum(sum)) = metric.data() else {
        panic!("expected an i64 up-down counter");
    };
    assert!(!sum.is_monotonic());
    assert_eq!(sum.data_points().count(), expected.len());
    for &(method, value) in expected {
        let attrs = method_attrs(method);
        let point = sum
            .data_points()
            .find(|point| has_attrs(point.attributes(), &attrs))
            .unwrap();
        assert_eq!(point.value(), value, "{method}");
    }
}

fn method_attrs(method: &'static str) -> Vec<KeyValue> {
    vec![
        KeyValue::new("rpc.system.name", "grpc"),
        KeyValue::new("rpc.method", method),
    ]
}

fn has_attrs<'a>(attrs: impl Iterator<Item = &'a KeyValue>, expected: &[KeyValue]) -> bool {
    let attrs: Vec<_> = attrs.collect();
    attrs.len() == expected.len() && expected.iter().all(|attr| attrs.contains(&attr))
}
