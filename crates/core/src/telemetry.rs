//! OpenTelemetry support for gRPC server metrics.
//!
//! Use [`TelemetryConfig::from_env`] to read deployment settings, then apply
//! setters to override them. Use [`TelemetryConfig::default`] to start without
//! reading configuration from the environment. Each enabled [`Telemetry`]
//! owns a provider; cloning shares that provider without installing it globally.
//!
//! Both `rpc.server.call.duration` and `rpc.server.active_requests` use
//! `rpc.system.name = "grpc"` and the canonical `package.Service/Method` as
//! `rpc.method` (`_OTHER` for unlisted paths). The duration histogram measures
//! seconds and deliberately uses a numeric `rpc.response.status_code` and,
//! for non-OK codes only, tonic's code name (such as `PermissionDenied`) as
//! `error.type`, rather than the string conventions in OpenTelemetry v1.37.
//! The existing `rpc` trace span is unchanged: it uses `rpc.system`, a separate
//! `rpc.service`, the method segment alone as `rpc.method`, and `grpc.status`
//! with tonic's debug name (such as `Ok`).
//!
//! A call completion observer receives a [`CallSummary`] exactly once per
//! tracked call, including cancellations. It runs on the serving path after
//! the built-in metrics are recorded, so it must be cheap and non-blocking.
//! Observer panics are caught and logged without changing the call outcome.

use std::sync::{Arc, Mutex};

use opentelemetry::{
    KeyValue,
    metrics::{Histogram, Meter, MeterProvider, noop::NoopMeterProvider},
};
use opentelemetry_sdk::{
    Resource,
    metrics::{SdkMeterProvider, reader::MetricReader},
};

use self::{buckets::validate_buckets, config::ExportMode, exporter::ShutdownRequest};

mod buckets;
mod call;
mod config;
mod exporter;
pub(crate) mod label;
pub(crate) mod layer;
pub(crate) mod recorder;
#[cfg(test)]
mod tests;

pub use buckets::DEFAULT_DURATION_BUCKETS;
pub use call::CallSummary;
pub use config::TelemetryConfig;
pub use opentelemetry;

const METER_NAME: &str = "rust_grpc_lib";
const LOG_TARGET: &str = "rust_grpc_lib::telemetry";

/// An explicitly owned metrics pipeline and its meter.
///
/// Independent instances do not share providers. Clones share the same
/// pipeline, including its shutdown state.
#[derive(Clone)]
pub struct Telemetry {
    inner: Arc<TelemetryInner>,
}

struct TelemetryInner {
    meter: Meter,
    bounds: Vec<f64>,
    shutdown: Mutex<Option<ShutdownRequest>>,
}

impl Telemetry {
    /// Read server metrics settings from the environment and start telemetry.
    ///
    /// Like [`Self::start`], this works with or without a Tokio runtime.
    #[must_use = "keep the telemetry handle to use its meter and shut down its provider"]
    pub fn from_env() -> Self {
        Self::start(TelemetryConfig::from_env())
    }

    /// Start telemetry from resolved settings, with or without a Tokio runtime.
    ///
    /// An enabled endpoint starts an OTLP/HTTP exporter on a background
    /// pipeline. No connection is required at startup. If the SDK is disabled,
    /// no endpoint is set, or setup fails, this returns a no-op meter.
    #[must_use = "keep the telemetry handle to use its meter and shut down its provider"]
    pub fn start(config: TelemetryConfig) -> Self {
        let ExportMode::Otlp(endpoint) = config.export_mode() else {
            return Self::noop(config.duration_buckets);
        };
        let endpoint = endpoint.to_owned();
        let interval = config.export_interval;
        Self::start_with_reader(move || exporter::otlp_reader(&endpoint, interval), config)
    }

    /// Create a no-op meter with the default duration bounds and no exporter.
    #[must_use = "this returns a no-op telemetry handle without changing any existing provider"]
    pub fn disabled() -> Self {
        Self::noop(DEFAULT_DURATION_BUCKETS.to_vec())
    }

    /// Whether this instance has a real provider that has not begun shutdown.
    #[must_use = "this only reports whether the provider is active"]
    pub fn is_enabled(&self) -> bool {
        self.inner.shutdown.lock().unwrap().is_some()
    }

    /// Get a cheap clone of the meter, which does not export when disabled.
    #[must_use = "use the returned meter to create instruments"]
    pub fn meter(&self) -> Meter {
        self.inner.meter.clone()
    }

    /// Build a seconds-unit histogram with the configured duration bounds.
    #[must_use = "use the returned histogram to record durations"]
    pub fn duration_histogram(
        &self,
        name: &'static str,
        description: &'static str,
    ) -> Histogram<f64> {
        self.build_histogram(name, description, &self.inner.bounds)
    }

    /// Build a seconds-unit histogram with bounds for this instrument only.
    ///
    /// Invalid bounds warn and fall back to [`DEFAULT_DURATION_BUCKETS`].
    #[must_use = "use the returned histogram to record durations"]
    pub fn duration_histogram_with(
        &self,
        name: &'static str,
        description: &'static str,
        bounds: &[f64],
    ) -> Histogram<f64> {
        let bounds = if validate_buckets(bounds) {
            bounds
        } else {
            tracing::warn!(target: LOG_TARGET, "invalid duration buckets; using defaults");
            &DEFAULT_DURATION_BUCKETS
        };
        self.build_histogram(name, description, bounds)
    }

    /// Flush and stop the shared provider, waiting at most five seconds.
    ///
    /// Requires a Tokio runtime with time enabled when an active provider is
    /// present. Only the first call starts shutdown; calls on other clones do
    /// not wait. If the wait times out, cleanup continues in the background.
    /// Dropping the last handle without calling this starts best-effort cleanup
    /// without waiting for it.
    pub async fn shutdown(self) {
        let request = self.inner.shutdown.lock().unwrap().take();
        if let Some(request) = request {
            exporter::shutdown(request).await;
        }
    }

    // C12 also uses this path to install an in-memory reader.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn from_reader(reader: impl MetricReader, config: TelemetryConfig) -> Self {
        Self::start_with_reader(move || Ok(reader), config)
    }

    fn start_with_reader<R: MetricReader>(
        reader: impl FnOnce() -> Result<R, &'static str> + Send + 'static,
        config: TelemetryConfig,
    ) -> Self {
        // The SDK reads OTEL_RESOURCE_ATTRIBUTES. The resolved config wins
        // for service.name and, if explicitly provided, service.version.
        let mut resource =
            Resource::builder().with_attribute(KeyValue::new("service.name", config.service_name));
        if let Some(version) = config.service_version {
            resource = resource.with_attribute(KeyValue::new("service.version", version));
        }
        let resource = resource.build();
        let bounds = config.duration_buckets;
        match exporter::start(move || {
            Ok(SdkMeterProvider::builder()
                .with_reader(reader()?)
                .with_resource(resource)
                .build())
        }) {
            Ok((meter, request)) => Self::new(meter, bounds, Some(request)),
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
        Self::new(Self::noop_meter(), bounds, None)
    }

    fn new(meter: Meter, bounds: Vec<f64>, shutdown: Option<ShutdownRequest>) -> Self {
        Self {
            inner: Arc::new(TelemetryInner {
                meter,
                bounds,
                shutdown: Mutex::new(shutdown),
            }),
        }
    }

    fn build_histogram(
        &self,
        name: &'static str,
        description: &'static str,
        bounds: &[f64],
    ) -> Histogram<f64> {
        self.inner
            .meter
            .f64_histogram(name)
            .with_description(description)
            .with_unit("s")
            .with_boundaries(bounds.to_vec())
            .build()
    }
}
