//! OTLP/HTTP setup and off-runtime provider lifecycle.

use std::{sync::mpsc, thread, time::Duration};

use opentelemetry::metrics::{Meter, MeterProvider};
use opentelemetry_otlp::{MetricExporter, WithExportConfig};
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};
use tokio::sync::oneshot;
use tonic::codegen::http::Uri;

use super::{LOG_TARGET, METER_NAME};

#[cfg(test)]
mod tests;

const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

pub(super) type ProviderShutdownSender = oneshot::Sender<oneshot::Sender<bool>>;

pub(super) fn otlp_reader(
    endpoint: &str,
    interval: Duration,
) -> Result<PeriodicReader<MetricExporter>, &'static str> {
    // The exporter only validates URI syntax, which also admits relative URLs
    // and non-HTTP schemes that would fail on every export.
    let uri: Uri = endpoint.parse().map_err(|_| "invalid OTLP/HTTP endpoint")?;
    if !matches!(uri.scheme_str(), Some("http" | "https")) || uri.host().is_none() {
        return Err("invalid OTLP/HTTP endpoint");
    }
    let exporter = MetricExporter::builder()
        .with_http()
        .with_endpoint(endpoint)
        .build()
        // Exporter errors can contain URLs or transport settings with secrets.
        .map_err(|_| "could not build OTLP/HTTP exporter")?;
    Ok(PeriodicReader::builder(exporter)
        .with_interval(interval)
        .build())
}

pub(super) fn start_provider_worker(
    build_provider: impl FnOnce() -> Result<SdkMeterProvider, &'static str> + Send + 'static,
) -> Result<(Meter, ProviderShutdownSender), &'static str> {
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<oneshot::Sender<bool>>();
    thread::Builder::new()
        .name("grpc-telemetry".into())
        .spawn(move || {
            let provider = match build_provider() {
                Ok(provider) => provider,
                Err(error) => {
                    let _ = ready_tx.send(Err(error));
                    return;
                }
            };
            let _ = ready_tx.send(Ok(provider.meter(METER_NAME)));
            // Keep provider teardown off Tokio even if the last handle drops
            // without sending a shutdown request.
            if let Ok(reply) = shutdown_rx.blocking_recv() {
                let flushed = provider.force_flush().is_ok();
                let stopped = provider.shutdown().is_ok();
                drop(provider);
                let _ = reply.send(flushed && stopped);
            } else {
                let _ = provider.shutdown();
            }
        })
        .map_err(|_| "could not spawn telemetry thread")?;
    let meter = ready_rx
        .recv()
        .map_err(|_| "telemetry thread stopped during startup")??;
    Ok((meter, shutdown_tx))
}

pub(super) async fn flush_and_shutdown_provider(shutdown_tx: ProviderShutdownSender) {
    let (reply, result) = oneshot::channel();
    if shutdown_tx.send(reply).is_err() {
        tracing::warn!(target: LOG_TARGET, "telemetry shutdown failed: lifecycle thread stopped");
        return;
    }
    match tokio::time::timeout(SHUTDOWN_TIMEOUT, result).await {
        Ok(Ok(true)) => {}
        Ok(Ok(false) | Err(_)) => {
            tracing::warn!(target: LOG_TARGET, "telemetry flush or shutdown failed")
        }
        Err(_) => {
            tracing::warn!(target: LOG_TARGET, "telemetry shutdown timed out after five seconds")
        }
    }
}
