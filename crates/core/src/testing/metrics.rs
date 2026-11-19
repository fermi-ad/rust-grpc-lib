//! On-demand collection of SDK metrics for test assertions.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, Weak},
    time::Duration,
};

use opentelemetry::KeyValue;
use opentelemetry_sdk::{
    error::OTelSdkResult,
    metrics::{
        InstrumentKind, ManualReader, Pipeline, Temporality,
        data::{AggregatedMetrics, Histogram, MetricData, ResourceMetrics, Sum},
        reader::MetricReader,
    },
};
use tonic::Code;

use crate::telemetry::{Telemetry, TelemetryConfig};

#[cfg(test)]
mod tests;

const CALL_DURATION: &str = "rpc.server.call.duration";
const CALL_TIMEOUT: Duration = Duration::from_secs(5);
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// An isolated SDK metrics pipeline with no network exporter or global state.
///
/// Reads collect cumulative data on demand, with no background collection
/// interval. Shutting down the telemetry retains its final collection for
/// inspection. The runtime scheduling probe is disabled for this pipeline.
///
/// ```
/// use rust_grpc_lib::testing::InMemory;
/// use rust_grpc_lib::telemetry::opentelemetry::KeyValue;
///
/// let metrics = InMemory::new();
/// let counter = metrics.telemetry().meter().u64_counter("jobs.completed").build();
/// counter.add(2, &[KeyValue::new("queue", "default")]);
///
/// let points = metrics.counters("jobs.completed");
/// assert_eq!(points[0].value, 2.0);
/// assert_eq!(points[0].attributes["queue"], "default");
/// ```
pub struct InMemory {
    telemetry: Telemetry,
    reader: SnapshotReader,
}

/// An explicit-bucket histogram point for one attribute set.
/// Integer measurements are converted to `f64` for the sum and extrema.
#[derive(Debug, Clone, PartialEq)]
pub struct HistogramSnapshot {
    pub unit: String,
    /// Point attributes with values rendered as strings.
    pub attributes: BTreeMap<String, String>,
    pub count: u64,
    pub sum: f64,
    pub min: Option<f64>,
    pub max: Option<f64>,
    /// The inclusive upper bounds of the finite buckets.
    pub bounds: Vec<f64>,
    /// Counts for each bound, followed by the count above the last bound.
    pub bucket_counts: Vec<u64>,
}

/// A counter or up-down counter point for one attribute set.
#[derive(Debug, Clone, PartialEq)]
pub struct CounterSnapshot {
    /// Point attributes with values rendered as strings.
    pub attributes: BTreeMap<String, String>,
    /// The cumulative sum. Large integer values may lose precision as `f64`.
    pub value: f64,
}

impl InMemory {
    /// Start a fresh pipeline without reading exporter settings or requiring a runtime.
    #[must_use = "keep the in-memory pipeline to inspect its metrics"]
    pub fn new() -> Self {
        let reader = SnapshotReader::default();
        let telemetry = Telemetry::from_reader(
            reader.clone(),
            TelemetryConfig::default().runtime_probe(false),
        );
        assert!(
            telemetry.is_enabled(),
            "could not start in-memory telemetry"
        );
        Self { telemetry, reader }
    }

    /// Clone the telemetry handle for writing into this pipeline.
    #[must_use = "use the handle to record metrics or supply it to the server"]
    pub fn telemetry(&self) -> Telemetry {
        self.telemetry.clone()
    }

    /// Collect explicit-bucket histogram points with this instrument name.
    #[must_use = "inspect the returned histogram points"]
    pub fn histograms(&self, name: &str) -> Vec<HistogramSnapshot> {
        self.reader.with_metrics(|export| {
            let mut points = Vec::new();
            for metric in export.scope_metrics().flat_map(|scope| scope.metrics()) {
                if metric.name() != name {
                    continue;
                }
                let unit = metric.unit();
                match metric.data() {
                    AggregatedMetrics::F64(MetricData::Histogram(data)) => {
                        points.extend(histogram_points(data, unit, |n| n));
                    }
                    AggregatedMetrics::U64(MetricData::Histogram(data)) => {
                        points.extend(histogram_points(data, unit, |n| n as f64));
                    }
                    AggregatedMetrics::I64(MetricData::Histogram(data)) => {
                        points.extend(histogram_points(data, unit, |n| n as f64));
                    }
                    _ => {}
                }
            }
            points.sort_by(|a, b| a.attributes.cmp(&b.attributes).then(a.unit.cmp(&b.unit)));
            points
        })
    }

    /// Collect counter and up-down counter points with this instrument name.
    #[must_use = "inspect the returned counter points"]
    pub fn counters(&self, name: &str) -> Vec<CounterSnapshot> {
        self.reader.with_metrics(|export| {
            let mut points = Vec::new();
            for metric in export.scope_metrics().flat_map(|scope| scope.metrics()) {
                if metric.name() != name {
                    continue;
                }
                match metric.data() {
                    AggregatedMetrics::F64(MetricData::Sum(data)) => {
                        points.extend(counter_points(data, |n| n));
                    }
                    AggregatedMetrics::U64(MetricData::Sum(data)) => {
                        points.extend(counter_points(data, |n| n as f64));
                    }
                    AggregatedMetrics::I64(MetricData::Sum(data)) => {
                        points.extend(counter_points(data, |n| n as f64));
                    }
                    _ => {}
                }
            }
            points.sort_by(|a, b| {
                a.attributes
                    .cmp(&b.attributes)
                    .then(a.value.total_cmp(&b.value))
            });
            points
        })
    }

    /// Wait for at least one completed call and return the current cumulative count.
    /// Use the canonical `package.Service/Method` (without a leading slash), or
    /// `_OTHER` for unlisted paths. Panics after five seconds if no call matches.
    pub async fn calls(&self, method: &str, code: Code) -> u64 {
        let status = (code as i64).to_string();
        self.poll(
            CALL_TIMEOUT,
            &format!("calls for {method:?} with {code:?}"),
            || {
                let count = self
                    .histograms(CALL_DURATION)
                    .iter()
                    .filter(|point| {
                        point
                            .attributes
                            .get("rpc.method")
                            .is_some_and(|m| m == method)
                            && point.attributes.get("rpc.response.status_code") == Some(&status)
                    })
                    .map(|point| point.count)
                    .sum();
                (count > 0).then_some(count)
            },
        )
        .await
    }

    /// Poll a predicate until it succeeds. The predicate is checked immediately
    /// and can use the snapshot methods to check the latest measurements.
    /// Panics with the recorded series if the timeout expires.
    pub async fn wait_until(&self, f: impl Fn(&Self) -> bool, timeout: Duration) {
        self.poll(timeout, "metric predicate", || f(self).then_some(()))
            .await;
    }

    async fn poll<T>(&self, timeout: Duration, what: &str, read: impl Fn() -> Option<T>) -> T {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if let Some(result) = read() {
                return result;
            }
            let now = tokio::time::Instant::now();
            if now >= deadline {
                break;
            }
            tokio::time::sleep_until((now + POLL_INTERVAL).min(deadline)).await;
        }
        let series = self.reader.with_metrics(|export| {
            let mut names: Vec<_> = export
                .scope_metrics()
                .flat_map(|scope| scope.metrics())
                .map(|metric| format!("{}: {:?}", metric.name(), metric.data()))
                .collect();
            names.sort();
            names
        });
        panic!("timed out after {timeout:?} waiting for {what}; recorded series: {series:?}");
    }
}

impl Default for InMemory {
    fn default() -> Self {
        Self::new()
    }
}

// ManualReader collects directly rather than polling or buffering exports. The
// shared lock serializes reads with shutdown so no read observes a closed reader.
#[derive(Clone, Debug, Default)]
struct SnapshotReader(Arc<ReaderInner>);

#[derive(Debug, Default)]
struct ReaderInner {
    reader: ManualReader,
    final_metrics: Mutex<Option<ResourceMetrics>>,
}

impl SnapshotReader {
    fn with_metrics<T>(&self, inspect: impl FnOnce(&ResourceMetrics) -> T) -> T {
        let final_metrics = self
            .0
            .final_metrics
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(metrics) = final_metrics.as_ref() {
            return inspect(metrics);
        }
        let mut metrics = ResourceMetrics::default();
        self.collect(&mut metrics)
            .expect("could not collect in-memory metrics");
        inspect(&metrics)
    }
}

impl MetricReader for SnapshotReader {
    fn register_pipeline(&self, pipeline: Weak<Pipeline>) {
        self.0.reader.register_pipeline(pipeline);
    }

    fn collect(&self, metrics: &mut ResourceMetrics) -> OTelSdkResult {
        self.0.reader.collect(metrics)
    }

    fn force_flush(&self) -> OTelSdkResult {
        self.0.reader.force_flush()
    }

    fn shutdown_with_timeout(&self, timeout: Duration) -> OTelSdkResult {
        let mut final_metrics = self
            .0
            .final_metrics
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut metrics = ResourceMetrics::default();
        let collected = self.collect(&mut metrics);
        *final_metrics = Some(metrics);
        collected.and(self.0.reader.shutdown_with_timeout(timeout))
    }

    fn temporality(&self, kind: InstrumentKind) -> Temporality {
        self.0.reader.temporality(kind)
    }
}

fn histogram_points<'a, T: Copy + 'a>(
    data: &'a Histogram<T>,
    unit: &'a str,
    number: impl Fn(T) -> f64 + 'a,
) -> impl Iterator<Item = HistogramSnapshot> + 'a {
    data.data_points().map(move |point| HistogramSnapshot {
        unit: unit.to_owned(),
        attributes: attributes(point.attributes()),
        count: point.count(),
        sum: number(point.sum()),
        min: point.min().map(&number),
        max: point.max().map(&number),
        bounds: point.bounds().collect(),
        bucket_counts: point.bucket_counts().collect(),
    })
}

fn counter_points<T: Copy>(
    data: &Sum<T>,
    number: impl Fn(T) -> f64,
) -> impl Iterator<Item = CounterSnapshot> {
    data.data_points().map(move |point| CounterSnapshot {
        attributes: attributes(point.attributes()),
        value: number(point.value()),
    })
}

fn attributes<'a>(values: impl Iterator<Item = &'a KeyValue>) -> BTreeMap<String, String> {
    values
        .map(|kv| (kv.key.as_str().to_owned(), kv.value.as_str().into_owned()))
        .collect()
}
