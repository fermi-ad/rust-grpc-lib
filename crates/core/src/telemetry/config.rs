//! Server telemetry settings and environment resolution.

use std::{env, fmt, path::Path, time::Duration};

use super::LOG_TARGET;

#[cfg(test)]
mod tests;

const DEFAULT_EXPORT_INTERVAL: Duration = Duration::from_secs(10);

/// Default duration histogram boundaries in seconds, from 10 µs to 7 s.
pub const DEFAULT_DURATION_BUCKET_BOUNDS_SECONDS: [f64; 36] = [
    1e-5, 1.5e-5, 2e-5, 3e-5, 5e-5, 7e-5, 1e-4, 1.5e-4, 2e-4, 3e-4, 5e-4, 7e-4, 1e-3, 1.5e-3, 2e-3,
    3e-3, 5e-3, 7e-3, 1e-2, 1.5e-2, 2e-2, 3e-2, 5e-2, 7e-2, 1e-1, 1.5e-1, 2e-1, 3e-1, 5e-1, 7e-1,
    1.0, 1.5, 2.0, 3.0, 5.0, 7.0,
];

/// Constructing it does not start an exporter. [`Self::default`] reads no
/// environment variables; the default interval is 10 seconds and the fallback
/// service name is `unknown_service:<executable>`. [`Self::from_env`] snapshots
/// the environment; later setters override it.
///
/// ```
/// use rust_grpc_lib::telemetry::TelemetryConfig;
///
/// let config = TelemetryConfig::from_env().service_name("my-service");
/// ```
#[derive(Clone)]
pub struct TelemetryConfig {
    endpoint: Option<String>,
    sdk_disabled: bool,
    pub(super) export_interval: Duration,
    pub(super) service_name: String,
    pub(super) service_version: Option<String>,
    pub(super) duration_buckets: Vec<f64>,
    pub(super) runtime_probe: bool,
}

impl TelemetryConfig {
    /// Reads these environment variables:
    ///
    /// | Variable | Effect |
    /// | --- | --- |
    /// | `OTEL_EXPORTER_OTLP_METRICS_ENDPOINT` | Full OTLP/HTTP metrics URL. Takes precedence over `OTEL_EXPORTER_OTLP_ENDPOINT`; an empty value is ignored. |
    /// | `OTEL_EXPORTER_OTLP_ENDPOINT` | Base OTLP/HTTP URL. Appends `/v1/metrics` when the metrics-specific endpoint is unset. An empty value is ignored. |
    /// | `OTEL_SDK_DISABLED` | `true` (case-insensitive) disables export, even with an endpoint. |
    /// | `OTEL_METRIC_EXPORT_INTERVAL` | Positive integer milliseconds; zero or invalid values use the 10-second default. |
    /// | `OTEL_SERVICE_NAME` | Non-empty name, overriding `service.name` from the resource attributes. |
    /// | `OTEL_RESOURCE_ATTRIBUTES` | A non-empty `service.name` is used if `OTEL_SERVICE_NAME` is unset. |
    /// | `GRPC_DURATION_BUCKETS` | Comma-separated boundaries in seconds. Invalid lists warn and use the defaults. |
    /// | `GRPC_RUNTIME_PROBE` | `true` or `false` (case-insensitive); other values warn and use the default. |
    #[must_use = "this returns the telemetry configuration without starting an exporter"]
    pub fn from_env() -> Self {
        Self::from_lookup(|key| env::var(key).ok())
    }

    /// An empty URL clears the endpoint.
    #[must_use = "this consumes the config and returns an updated one"]
    pub fn endpoint(mut self, url: impl Into<String>) -> Self {
        self.endpoint = Some(url.into()).filter(|url| !url.is_empty());
        self
    }

    /// Disables export regardless of the endpoint.
    #[must_use = "this consumes the config and returns an updated one"]
    pub fn sdk_disabled(mut self, disabled: bool) -> Self {
        self.sdk_disabled = disabled;
        self
    }

    /// Zero restores the 10-second default.
    #[must_use = "this consumes the config and returns an updated one"]
    pub fn export_interval(mut self, interval: Duration) -> Self {
        self.export_interval = if interval.is_zero() {
            DEFAULT_EXPORT_INTERVAL
        } else {
            interval
        };
        self
    }

    /// An empty name restores the executable-based fallback.
    #[must_use = "this consumes the config and returns an updated one"]
    pub fn service_name(mut self, name: impl Into<String>) -> Self {
        let name = name.into();
        self.service_name = if name.is_empty() {
            default_service_name()
        } else {
            name
        };
        self
    }

    /// No version is set unless you call this.
    #[must_use = "this consumes the config and returns an updated one"]
    pub fn service_version(mut self, version: impl Into<String>) -> Self {
        self.service_version = Some(version.into());
        self
    }

    /// Boundaries (seconds) must be non-empty, finite, positive, and strictly
    /// increasing; otherwise a warning is logged and
    /// [`DEFAULT_DURATION_BUCKET_BOUNDS_SECONDS`] is restored.
    #[must_use = "this consumes the config and returns an updated one"]
    pub fn duration_bucket_bounds_seconds(mut self, bounds: impl Into<Vec<f64>>) -> Self {
        let bounds = bounds.into();
        self.duration_buckets = if valid_buckets(&bounds) {
            bounds
        } else {
            tracing::warn!(target: LOG_TARGET, "invalid duration buckets; using defaults");
            DEFAULT_DURATION_BUCKET_BOUNDS_SECONDS.to_vec()
        };
        self
    }

    /// Records `runtime.scheduling_delay`; on by default when metrics are
    /// active, one probe per pipeline on the first serving runtime. On a
    /// multithreaded runtime it detects broad starvation, not one blocked worker.
    #[must_use = "this consumes the config and returns an updated one"]
    pub fn runtime_scheduling_probe(mut self, enabled: bool) -> Self {
        self.runtime_probe = enabled;
        self
    }

    pub(super) fn export_endpoint(&self) -> Option<&str> {
        if self.sdk_disabled {
            None
        } else {
            self.endpoint.as_deref()
        }
    }

    pub(super) fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Self {
        let non_empty = |key| lookup(key).filter(|value| !value.is_empty());
        let endpoint = non_empty("OTEL_EXPORTER_OTLP_METRICS_ENDPOINT")
            .or_else(|| non_empty("OTEL_EXPORTER_OTLP_ENDPOINT").map(metrics_url));
        let sdk_disabled =
            lookup("OTEL_SDK_DISABLED").is_some_and(|value| value.eq_ignore_ascii_case("true"));
        let export_interval = lookup("OTEL_METRIC_EXPORT_INTERVAL")
            .and_then(|value| value.parse::<u64>().ok())
            .map(Duration::from_millis)
            .unwrap_or_default();
        let service_name = non_empty("OTEL_SERVICE_NAME")
            .or_else(|| {
                lookup("OTEL_RESOURCE_ATTRIBUTES")
                    .as_deref()
                    .and_then(resource_service_name)
            })
            .unwrap_or_else(default_service_name);
        let runtime_probe = match lookup("GRPC_RUNTIME_PROBE") {
            Some(value) if value.eq_ignore_ascii_case("false") => false,
            Some(value) if !value.eq_ignore_ascii_case("true") => {
                tracing::warn!(target: LOG_TARGET, "invalid GRPC_RUNTIME_PROBE; using default");
                true
            }
            _ => true,
        };
        let config = Self {
            endpoint,
            sdk_disabled,
            runtime_probe,
            service_name,
            service_version: None,
            duration_buckets: DEFAULT_DURATION_BUCKET_BOUNDS_SECONDS.to_vec(),
            export_interval: DEFAULT_EXPORT_INTERVAL,
        }
        .export_interval(export_interval);
        if let Some(value) = lookup("GRPC_DURATION_BUCKETS") {
            let bounds = value
                .split(',')
                .map(|part| part.trim().parse::<f64>())
                .collect::<Result<Vec<_>, _>>();
            config.duration_bucket_bounds_seconds(bounds.unwrap_or_default())
        } else {
            config
        }
    }
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self::from_lookup(|_| None)
    }
}

impl fmt::Debug for TelemetryConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // URLs may contain user info or query tokens. Never format the URL.
        f.debug_struct("TelemetryConfig")
            .field("endpoint_configured", &self.endpoint.is_some())
            .field("sdk_disabled", &self.sdk_disabled)
            .field("export_interval", &self.export_interval)
            .field("service_name", &self.service_name)
            .field("service_version", &self.service_version)
            .field("duration_buckets", &self.duration_buckets)
            .field("runtime_probe", &self.runtime_probe)
            .finish()
    }
}

fn valid_buckets(bounds: &[f64]) -> bool {
    !bounds.is_empty()
        && bounds.iter().all(|&bound| bound.is_finite() && bound > 0.0)
        && bounds.windows(2).all(|pair| pair[0] < pair[1])
}

fn metrics_url(mut base: String) -> String {
    if !base.ends_with('/') {
        base.push('/');
    }
    base.push_str("v1/metrics");
    base
}

fn default_service_name() -> String {
    env::current_exe()
        .ok()
        .as_deref()
        .and_then(Path::file_name)
        .map(|name| format!("unknown_service:{}", name.to_string_lossy()))
        .unwrap_or_else(|| String::from("unknown_service"))
}

fn resource_service_name(attributes: &str) -> Option<String> {
    // Match the SDK's whitespace and last-entry handling, and ignore empty names.
    attributes
        .rsplit(',')
        .filter_map(|entry| entry.split_once('='))
        .find(|(key, _)| key.trim() == "service.name")
        .map(|(_, value)| value.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}
