//! Probe timing, configuration, and task cleanup.

use super::*;
use crate::telemetry::TelemetryConfig;
use opentelemetry_sdk::metrics::{
    InMemoryMetricExporter, PeriodicReader,
    data::{AggregatedMetrics, HistogramDataPoint, MetricData, ResourceMetrics},
    reader::MetricReader,
};
use tokio::{task::yield_now, time::advance};

const BOUNDS: [f64; 2] = [0.005, 0.025];

#[tokio::test(start_paused = true)]
async fn probe_keeps_absolute_cadence_and_skips_missed_ticks() {
    let (telemetry, reader) =
        in_memory(TelemetryConfig::default().duration_bucket_bounds_seconds(BOUNDS));
    let _probe = telemetry.attach_runtime_probe().unwrap();
    yield_now().await;
    assert!(point(&reader).is_none());

    // Wake slightly late, then near the original second deadline. The first
    // delay must not shift the deadline for the next sample.
    elapse(SAMPLE_PERIOD + Duration::from_millis(4)).await;
    assert_eq!(point(&reader).unwrap().count(), 1);
    elapse(Duration::from_millis(7)).await;
    assert_eq!(point(&reader).unwrap().count(), 2);

    // Jump from t=21 ms to t=71 ms, overshooting the t=30 ms deadline by 41 ms.
    elapse(Duration::from_millis(50)).await;
    let delayed = point(&reader).unwrap();
    assert_eq!(delayed.count(), 3, "no burst of catch-up samples");
    assert!((delayed.max().unwrap() - 0.041).abs() < 1e-9);
    assert_eq!(delayed.bounds().collect::<Vec<_>>(), BOUNDS);
    assert_eq!(delayed.bucket_counts().collect::<Vec<_>>(), [2, 0, 1]);
    assert_eq!(delayed.attributes().count(), 0);

    elapse(SAMPLE_PERIOD - Duration::from_millis(1)).await;
    assert_eq!(point(&reader).unwrap().count(), 3);
    elapse(Duration::from_millis(2)).await;
    assert_eq!(point(&reader).unwrap().count(), 4);
}

#[tokio::test]
async fn probe_samples_on_a_live_runtime() {
    let (telemetry, reader) = in_memory(TelemetryConfig::default());
    let _probe = telemetry.attach_runtime_probe().unwrap();
    tokio::time::sleep(8 * SAMPLE_PERIOD).await;
    yield_now().await;
    let count = point(&reader).map_or(0, |point| point.count());
    assert!(
        (1..=16).contains(&count),
        "unexpected sample count: {count}"
    );
}

#[tokio::test]
async fn probe_records_a_blocked_runtime() {
    let (telemetry, reader) = in_memory(TelemetryConfig::default());
    // Block the only worker for five periods, starting before the first
    // sample. At most one period elapses before the first deadline.
    let _probe = tokio::spawn(async move {
        let probe = telemetry.attach_runtime_probe().unwrap();
        std::thread::sleep(5 * SAMPLE_PERIOD);
        probe
    })
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if point(&reader)
                .and_then(|point| point.max())
                .is_some_and(|delay| delay >= (4 * SAMPLE_PERIOD).as_secs_f64())
            {
                break;
            }
            tokio::time::sleep(SAMPLE_PERIOD).await;
        }
    })
    .await
    .expect("the blocked worker should produce a large scheduling delay");
}

#[test]
fn probe_opt_outs_do_not_spawn_even_without_a_tokio_runtime() {
    for telemetry in [
        Telemetry::start(TelemetryConfig::default()),
        Telemetry::start(
            TelemetryConfig::default()
                .endpoint("http://localhost")
                .sdk_disabled(true),
        ),
        Telemetry::start(TelemetryConfig::default().endpoint("invalid")),
    ] {
        assert!(!telemetry.is_enabled());
        assert!(telemetry.attach_runtime_probe().is_none());
    }

    for config in [
        TelemetryConfig::default().runtime_scheduling_probe(false),
        TelemetryConfig::from_lookup(|key| (key == "GRPC_RUNTIME_PROBE").then(|| "false".into())),
    ] {
        let (telemetry, _) = in_memory(config);
        assert!(telemetry.is_enabled());
        assert!(telemetry.attach_runtime_probe().is_none());
    }
}

#[tokio::test(start_paused = true)]
async fn probe_stops_before_provider_shutdown_even_with_an_active_server() {
    let (telemetry, reader) = in_memory(TelemetryConfig::default());
    let probe_guard = telemetry.attach_runtime_probe().unwrap();
    let stop = stop_receiver(&telemetry);
    yield_now().await;
    elapse(SAMPLE_PERIOD + Duration::from_millis(1)).await;
    assert_eq!(point(&reader).unwrap().count(), 1);

    // Drive shutdown up to its first await. The stop flag must already be
    // set when shutdown hands teardown off to the provider thread.
    let shutdown = telemetry.clone().shutdown();
    tokio::pin!(shutdown);
    let _ = std::future::poll_fn(|cx| {
        let _ = shutdown.as_mut().poll(cx);
        std::task::Poll::Ready(())
    })
    .await;
    assert!(*stop.borrow());
    assert!(stop.has_changed().is_err(), "the stop sender was dropped");
    assert!(!telemetry.is_enabled());
    assert!(telemetry.attach_runtime_probe().is_none());
    drop(probe_guard);
    shutdown.await;
}

fn stop_receiver(telemetry: &Telemetry) -> watch::Receiver<bool> {
    telemetry
        .inner
        .lifecycle
        .lock()
        .unwrap()
        .probe
        .as_ref()
        .unwrap()
        .stop_tx
        .subscribe()
}

async fn elapse(duration: Duration) {
    advance(duration).await;
    yield_now().await;
}

fn in_memory(config: TelemetryConfig) -> (Telemetry, impl MetricReader) {
    let reader = PeriodicReader::builder(InMemoryMetricExporter::default())
        .with_interval(Duration::from_secs(600))
        .build();
    (Telemetry::from_reader(reader.clone(), config), reader)
}

fn point(reader: &impl MetricReader) -> Option<HistogramDataPoint<f64>> {
    let mut metrics = ResourceMetrics::default();
    reader.collect(&mut metrics).unwrap();
    let metric = metrics
        .scope_metrics()
        .flat_map(|scope| scope.metrics())
        .find(|metric| metric.name() == "runtime.scheduling_delay")?;
    assert_eq!(metric.unit(), "s");
    let AggregatedMetrics::F64(MetricData::Histogram(histogram)) = metric.data() else {
        panic!("expected a seconds histogram");
    };
    let mut points = histogram.data_points();
    let point = points.next().unwrap().clone();
    assert!(points.next().is_none());
    Some(point)
}
