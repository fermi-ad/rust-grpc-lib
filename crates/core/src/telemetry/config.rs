//! Server telemetry settings and environment resolution.

use std::{env, fmt, path::Path, time::Duration};

use super::buckets::{DEFAULT_DURATION_BUCKETS, validate_buckets};

#[cfg(test)]
mod tests;

const DEFAULT_EXPORT_INTERVAL: Duration = Duration::from_secs(10);
const LOG_TARGET: &str = "rust_grpc_lib::telemetry";

/// Configuration for server metrics. Constructing it does not start an exporter.
///
/// [`Self::default`] reads no configuration variables: the SDK is not disabled,
/// but no endpoint is configured. The interval is 10 seconds, duration histograms
/// use [`DEFAULT_DURATION_BUCKETS`], and the runtime probe is enabled only when
/// metrics are enabled. The default service name is `unknown_service:<executable
/// file name>` (or `unknown_service` if unavailable), and no version is set.
///
/// [`Self::from_env`] snapshots the configuration variables. Setters called
/// afterward take precedence, and the last setter for a field wins. For service
/// names, the order is explicit setter, `OTEL_SERVICE_NAME`, `service.name` in
/// `OTEL_RESOURCE_ATTRIBUTES`, then the executable-based fallback.
#[derive(Clone)]
pub struct TelemetryConfig {
    endpoint: Option<String>,
    sdk_disabled: bool,
    pub(super) export_interval: Duration,
    pub(super) service_name: String,
    pub(super) service_version: Option<String>,
    pub(super) duration_buckets: Vec<f64>,
    runtime_probe: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum ExportMode<'a> {
    Disabled,
    Otlp(&'a str),
    NoOp,
}

impl TelemetryConfig {
    /// Read server metrics settings from the process environment.
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
    ///
    /// Use [`Self::service_version`] to set a version explicitly. This method
    /// does not read transport credentials or initialize an exporter.
    #[must_use = "this returns the telemetry configuration without starting an exporter"]
    pub fn from_env() -> Self {
        Self::from_lookup(|key| env::var(key).ok())
    }

    /// Set the full OTLP/HTTP metrics URL. An empty URL clears the endpoint.
    #[must_use = "this consumes the config and returns an updated one"]
    pub fn endpoint(mut self, url: impl Into<String>) -> Self {
        self.endpoint = Some(url.into()).filter(|url| !url.is_empty());
        self
    }

    /// Set whether the SDK is disabled, independently of the endpoint.
    #[must_use = "this consumes the config and returns an updated one"]
    pub fn sdk_disabled(mut self, disabled: bool) -> Self {
        self.sdk_disabled = disabled;
        self
    }

    /// Set the export interval. Zero restores the 10-second default.
    #[must_use = "this consumes the config and returns an updated one"]
    pub fn export_interval(mut self, interval: Duration) -> Self {
        self.export_interval = if interval.is_zero() {
            DEFAULT_EXPORT_INTERVAL
        } else {
            interval
        };
        self
    }

    /// Set the service name, overriding any name from the environment.
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

    /// Set the service version. No version is inferred from this crate.
    #[must_use = "this consumes the config and returns an updated one"]
    pub fn service_version(mut self, version: impl Into<String>) -> Self {
        self.service_version = Some(version.into());
        self
    }

    /// Set duration histogram boundaries in seconds.
    ///
    /// Boundaries must be non-empty, finite, positive, and strictly increasing.
    /// An invalid list logs a warning and restores [`DEFAULT_DURATION_BUCKETS`].
    #[must_use = "this consumes the config and returns an updated one"]
    pub fn duration_buckets(mut self, bounds: impl Into<Vec<f64>>) -> Self {
        let bounds = bounds.into();
        self.duration_buckets = if validate_buckets(&bounds) {
            bounds
        } else {
            tracing::warn!(target: LOG_TARGET, "invalid duration buckets; using defaults");
            DEFAULT_DURATION_BUCKETS.to_vec()
        };
        self
    }

    /// Set whether the runtime probe may run when metrics are enabled.
    /// This cannot enable the probe on a no-op meter.
    #[must_use = "this consumes the config and returns an updated one"]
    pub fn runtime_probe(mut self, enabled: bool) -> Self {
        self.runtime_probe = enabled;
        self
    }

    pub(super) fn export_mode(&self) -> ExportMode<'_> {
        if self.sdk_disabled {
            ExportMode::Disabled
        } else if let Some(endpoint) = &self.endpoint {
            ExportMode::Otlp(endpoint)
        } else {
            ExportMode::NoOp
        }
    }

    // C9 resolves the preference against the actual meter, including exporter failures.
    #[allow(dead_code)]
    pub(super) fn runtime_probe_enabled(&self, metrics_enabled: bool) -> bool {
        self.runtime_probe && metrics_enabled
    }

    fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Self {
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
            duration_buckets: DEFAULT_DURATION_BUCKETS.to_vec(),
            export_interval: DEFAULT_EXPORT_INTERVAL,
        }
        .export_interval(export_interval);
        if let Some(value) = lookup("GRPC_DURATION_BUCKETS") {
            let bounds = value
                .split(',')
                .map(|part| part.trim().parse::<f64>())
                .collect::<Result<Vec<_>, _>>();
            config.duration_buckets(bounds.unwrap_or_default())
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
