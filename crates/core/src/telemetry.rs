//! OpenTelemetry metrics for gRPC calls and runtime scheduling delay.
//! Configure OTLP/HTTP export with [`TelemetryConfig`] and share a provider via
//! [`Telemetry`] without installing it globally.

use std::sync::{Arc, Mutex};

use opentelemetry::{
    KeyValue,
    metrics::{Histogram, Meter, MeterProvider, noop::NoopMeterProvider},
};
use opentelemetry_sdk::{
    Resource,
    metrics::{SdkMeterProvider, reader::MetricReader},
};

use self::exporter::ProviderShutdownSender;

mod call;
mod config;
mod exporter;
pub(crate) mod label;
pub(crate) mod layer;
mod probe;
pub(crate) mod recorder;
#[cfg(test)]
mod tests;

pub use call::CallSummary;
pub use config::{DEFAULT_DURATION_BUCKET_BOUNDS_SECONDS, TelemetryConfig};
pub use opentelemetry;

const METER_NAME: &str = "rust_grpc_lib";
const LOG_TARGET: &str = "rust_grpc_lib::telemetry";

/// Clones share a meter and shutdown state.
#[derive(Clone)]
pub struct Telemetry {
    inner: Arc<TelemetryInner>,
}

struct TelemetryInner {
    meter: Meter,
    duration_bounds_seconds: Vec<f64>,
    runtime_probe: bool,
    lifecycle: Mutex<Lifecycle>,
}

struct Lifecycle {
    provider_shutdown: Option<ProviderShutdownSender>,
    probe: Option<probe::RunningProbe>,
}

impl Telemetry {
    /// Needs no Tokio runtime or live collector. The meter is a no-op if the
    /// SDK is disabled, no endpoint is set, or exporter setup fails.
    ///
    /// ```
    /// use rust_grpc_lib::telemetry::{Telemetry, TelemetryConfig};
    ///
    /// let telemetry = Telemetry::start(TelemetryConfig::from_env());
    /// let meter = telemetry.meter();
    /// ```
    #[must_use = "keep the telemetry handle to use its meter and shut down its provider"]
    pub fn start(config: TelemetryConfig) -> Self {
        let Some(endpoint) = config.export_endpoint().map(str::to_owned) else {
            return Self::noop(config.duration_buckets);
        };
        let interval = config.export_interval;
        Self::start_with_reader(move || exporter::otlp_reader(&endpoint, interval), config)
    }

    /// Whether a real provider is running, as opposed to a no-op meter.
    #[must_use = "this only reports whether the provider is active"]
    pub fn is_enabled(&self) -> bool {
        self.inner
            .lifecycle
            .lock()
            .unwrap()
            .provider_shutdown
            .is_some()
    }

    /// The meter to create instruments on.
    #[must_use = "use the returned meter to create instruments"]
    pub fn meter(&self) -> Meter {
        self.inner.meter.clone()
    }

    /// A histogram in seconds that uses the configured duration bounds.
    #[must_use = "use the returned histogram to record durations"]
    pub fn duration_histogram(
        &self,
        name: &'static str,
        description: &'static str,
    ) -> Histogram<f64> {
        self.inner
            .meter
            .f64_histogram(name)
            .with_description(description)
            .with_unit("s")
            .with_boundaries(self.inner.duration_bounds_seconds.clone())
            .build()
    }

    /// Waits up to five seconds for flush. Only the first caller waits; cleanup
    /// continues after a timeout or when the last handle drops without calling this.
    ///
    /// # Panics
    ///
    /// Shutting down an active provider requires a Tokio runtime with time enabled.
    pub async fn shutdown(self) {
        let request = {
            let mut lifecycle = self.inner.lifecycle.lock().unwrap();
            drop(lifecycle.probe.take());
            lifecycle.provider_shutdown.take()
        };
        if let Some(request) = request {
            exporter::flush_and_shutdown_provider(request).await;
        }
    }

    pub(crate) fn attach_runtime_probe(&self) -> Option<probe::ServerGuard> {
        let mut lifecycle = self.inner.lifecycle.lock().unwrap();
        if !self.inner.runtime_probe || lifecycle.provider_shutdown.is_none() {
            return None;
        }
        let running_probe = lifecycle.probe.get_or_insert_with(|| {
            probe::RunningProbe::start(self.duration_histogram(
                "runtime.scheduling_delay",
                "How late the Tokio runtime wakes a fixed-period timer",
            ))
        });
        running_probe.attached_servers += 1;
        Some(probe::ServerGuard(self.clone()))
    }

    #[cfg(test)]
    pub(crate) fn runtime_probe_server_count(&self) -> usize {
        self.inner
            .lifecycle
            .lock()
            .unwrap()
            .probe
            .as_ref()
            .map_or(0, |probe| probe.attached_servers)
    }

    #[cfg(any(test, feature = "test-util"))]
    pub(crate) fn from_reader(reader: impl MetricReader, config: TelemetryConfig) -> Self {
        Self::start_with_reader(move || Ok(reader), config)
    }

    fn start_with_reader<R: MetricReader>(
        build_reader: impl FnOnce() -> Result<R, &'static str> + Send + 'static,
        config: TelemetryConfig,
    ) -> Self {
        let runtime_probe = config.runtime_probe;
        // The SDK reads OTEL_RESOURCE_ATTRIBUTES. The resolved config wins
        // for service.name and, if explicitly provided, service.version.
        let mut resource =
            Resource::builder().with_attribute(KeyValue::new("service.name", config.service_name));
        if let Some(version) = config.service_version {
            resource = resource.with_attribute(KeyValue::new("service.version", version));
        }
        let resource = resource.build();
        let bounds = config.duration_buckets;
        match exporter::start_provider_worker(move || {
            Ok(SdkMeterProvider::builder()
                .with_reader(build_reader()?)
                .with_resource(resource)
                .build())
        }) {
            Ok((meter, request)) => Self::new(meter, bounds, runtime_probe, Some(request)),
            Err(reason) => {
                tracing::error!(target: LOG_TARGET, reason, "failed to start telemetry; using a no-op meter");
                Self::noop(bounds)
            }
        }
    }

    pub(crate) fn noop_meter() -> Meter {
        NoopMeterProvider::new().meter(METER_NAME)
    }

    fn noop(bounds: Vec<f64>) -> Self {
        Self::new(Self::noop_meter(), bounds, false, None)
    }

    fn new(
        meter: Meter,
        duration_bounds_seconds: Vec<f64>,
        runtime_probe: bool,
        provider_shutdown: Option<ProviderShutdownSender>,
    ) -> Self {
        Self {
            inner: Arc::new(TelemetryInner {
                meter,
                duration_bounds_seconds,
                runtime_probe,
                lifecycle: Mutex::new(Lifecycle {
                    provider_shutdown,
                    probe: None,
                }),
            }),
        }
    }
}
