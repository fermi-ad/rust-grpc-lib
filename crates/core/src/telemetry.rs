//! OpenTelemetry support for gRPC server metrics.
//!
//! Use [`TelemetryConfig::from_env`] to read deployment settings, then apply
//! setters to override them. Use [`TelemetryConfig::default`] to start without
//! reading configuration from the environment.

mod buckets;
mod config;
#[cfg(test)]
mod tests;

pub use buckets::DEFAULT_DURATION_BUCKETS;
pub use config::TelemetryConfig;
pub use opentelemetry;
