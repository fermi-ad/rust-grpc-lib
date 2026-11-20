use std::{
    io,
    sync::{Arc, Mutex},
    time::Duration,
};

use opentelemetry::{Key, Value};
use opentelemetry_sdk::metrics::{
    InMemoryMetricExporter, PeriodicReader,
    data::{AggregatedMetrics, MetricData, ResourceMetrics},
};
use tracing::subscriber::DefaultGuard;

use super::{
    DEFAULT_DURATION_BUCKET_BOUNDS_SECONDS, Telemetry, TelemetryConfig,
    opentelemetry::metrics::{MeterProvider, noop::NoopMeterProvider},
};

#[test]
fn reexport_exposes_metrics_api() {
    let meter = NoopMeterProvider::new().meter("telemetry-test");
    meter.u64_counter("test.calls").build().add(1, &[]);
}

#[tokio::test]
async fn disabled_paths_have_no_provider() {
    for telemetry in [
        Telemetry::start(TelemetryConfig::default()),
        Telemetry::start(
            TelemetryConfig::default()
                .endpoint("invalid")
                .sdk_disabled(true),
        ),
    ] {
        assert!(!telemetry.is_enabled());
        telemetry
            .meter()
            .u64_counter("test.calls")
            .build()
            .add(1, &[]);
        telemetry
            .duration_histogram("test.duration", "duration")
            .record(0.1, &[]);
        telemetry.clone().shutdown().await;
        telemetry.shutdown().await;
    }
}

#[tokio::test]
async fn duration_histogram_exports_default_bounds_and_seconds() {
    let (telemetry, exporter) = in_memory(TelemetryConfig::default());
    telemetry
        .duration_histogram("test.duration", "duration")
        .record(0.015, &[]);
    telemetry.shutdown().await;
    let exported = exporter.get_finished_metrics().unwrap();
    let mut counts = vec![0; DEFAULT_DURATION_BUCKET_BOUNDS_SECONDS.len() + 1];
    counts[19] = 1;
    assert_histogram(
        &exported[0],
        "test.duration",
        &DEFAULT_DURATION_BUCKET_BOUNDS_SECONDS,
        &counts,
    );
}

#[tokio::test]
async fn configured_bounds_apply_to_duration_histograms() {
    let (telemetry, exporter) =
        in_memory(TelemetryConfig::default().duration_bucket_bounds_seconds([0.5, 1.0]));
    telemetry
        .duration_histogram("test.configured", "duration")
        .record(0.5, &[]);
    telemetry.shutdown().await;

    let exported = exporter.get_finished_metrics().unwrap();
    assert_histogram(&exported[0], "test.configured", &[0.5, 1.0], &[1, 0, 0]);
}

#[tokio::test]
async fn providers_are_isolated_and_shutdown_is_shared_by_clones() {
    let (first, first_exporter) = in_memory(
        TelemetryConfig::default()
            .service_name("first")
            .service_version("1"),
    );
    let (second, second_exporter) = in_memory(
        TelemetryConfig::default()
            .service_name("second")
            .service_version("2"),
    );
    let clone = first.clone();
    assert!(clone.is_enabled());
    first.meter().u64_counter("test.first").build().add(1, &[]);
    second
        .meter()
        .u64_counter("test.second")
        .build()
        .add(2, &[]);
    first.shutdown().await;
    assert!(!clone.is_enabled());
    assert!(second.is_enabled());
    let exports = first_exporter.get_finished_metrics().unwrap().len();
    clone.shutdown().await;
    assert_eq!(
        first_exporter.get_finished_metrics().unwrap().len(),
        exports
    );
    second.shutdown().await;

    for (exporter, service, version, name) in [
        (first_exporter, "first", "1", "test.first"),
        (second_exporter, "second", "2", "test.second"),
    ] {
        let exported = exporter.get_finished_metrics().unwrap();
        let resource = exported[0].resource();
        assert_eq!(
            resource.get(&Key::new("service.name")),
            Some(Value::from(service))
        );
        assert_eq!(
            resource.get(&Key::new("service.version")),
            Some(Value::from(version))
        );
        let names: Vec<_> = exported[0]
            .scope_metrics()
            .flat_map(|scope| scope.metrics())
            .map(|metric| metric.name())
            .collect();
        assert_eq!(names, [name]);
    }
}

fn in_memory(config: TelemetryConfig) -> (Telemetry, InMemoryMetricExporter) {
    let exporter = InMemoryMetricExporter::default();
    let reader = PeriodicReader::builder(exporter.clone())
        .with_interval(Duration::from_secs(600))
        .build();
    (Telemetry::from_reader(reader, config), exporter)
}

fn assert_histogram(export: &ResourceMetrics, name: &str, bounds: &[f64], counts: &[u64]) {
    let metric = export
        .scope_metrics()
        .flat_map(|scope| scope.metrics())
        .find(|metric| metric.name() == name)
        .unwrap();
    assert_eq!(metric.unit(), "s");
    assert_eq!(metric.description(), "duration");
    let AggregatedMetrics::F64(MetricData::Histogram(histogram)) = metric.data() else {
        panic!("expected a duration histogram");
    };
    let point = histogram.data_points().next().unwrap();
    assert_eq!(point.count(), 1);
    assert_eq!(point.bounds().collect::<Vec<_>>(), bounds);
    assert_eq!(point.bucket_counts().collect::<Vec<_>>(), counts);
}

#[derive(Clone, Default)]
pub(super) struct Capture(Arc<Mutex<Vec<u8>>>);

impl Capture {
    pub(super) fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

impl io::Write for Capture {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(super) fn capture_logs() -> (Capture, DefaultGuard) {
    let capture = Capture::default();
    let sink = capture.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || sink.clone())
        .with_ansi(false)
        .finish();
    (capture, tracing::subscriber::set_default(subscriber))
}
