//! Built-in server call metrics and completion observer.

use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Arc,
};

use opentelemetry::{
    KeyValue,
    metrics::{Histogram, UpDownCounter},
};
use tokio::sync::watch;
use tonic::Code;

use super::{CallSummary, LOG_TARGET, Telemetry};

#[cfg(test)]
mod tests;

pub(crate) type CallCompletionObserver = Arc<dyn Fn(&CallSummary) + Send + Sync>;

pub(crate) struct CallRecorder {
    call_duration: Histogram<f64>,
    in_flight_calls: UpDownCounter<i64>,
    on_call: Option<CallCompletionObserver>,
    // Keep the shutdown barrier open until the last call guard records its outcome.
    _call_drain_guard: watch::Receiver<()>,
}

impl CallRecorder {
    pub(crate) fn new(
        telemetry: &Telemetry,
        on_call: Option<CallCompletionObserver>,
        call_drain_guard: watch::Receiver<()>,
    ) -> Self {
        Self {
            call_duration: telemetry.duration_histogram(
                "rpc.server.call.duration",
                "Duration of incoming gRPC calls",
            ),
            in_flight_calls: telemetry
                .meter()
                .i64_up_down_counter("rpc.server.active_requests")
                .with_description("Number of in-flight gRPC calls")
                .with_unit("{request}")
                .build(),
            on_call,
            _call_drain_guard: call_drain_guard,
        }
    }

    pub(super) fn started(&self, method: Option<&'static str>) {
        self.in_flight_calls.add(1, &method_attributes(method));
    }

    pub(super) fn finished(&self, summary: CallSummary) {
        let attributes = method_attributes(summary.method);
        self.in_flight_calls.add(-1, &attributes);

        let [system, method] = attributes;
        // This metric uses numeric gRPC status codes and tonic error names,
        // not the OpenTelemetry v1.37 string conventions.
        let status = KeyValue::new("rpc.response.status_code", summary.status as i64);
        let duration_seconds = summary.duration.as_secs_f64();
        if summary.status == Code::Ok {
            self.call_duration
                .record(duration_seconds, &[system, method, status]);
        } else {
            let error = KeyValue::new("error.type", format!("{:?}", summary.status));
            self.call_duration
                .record(duration_seconds, &[system, method, status, error]);
        }

        if let Some(on_call) = &self.on_call
            && let Err(payload) = catch_unwind(AssertUnwindSafe(|| on_call(&summary)))
        {
            tracing::error!(target: LOG_TARGET, "gRPC call observer panicked");
            // A custom panic payload can panic on drop; leak a second payload
            // rather than risking another unwind during call completion.
            if let Err(payload) = catch_unwind(AssertUnwindSafe(|| drop(payload))) {
                std::mem::forget(payload);
            }
        }
    }
}

fn method_attributes(method: Option<&'static str>) -> [KeyValue; 2] {
    [
        KeyValue::new("rpc.system.name", "grpc"),
        KeyValue::new("rpc.method", method.unwrap_or("_OTHER")),
    ]
}
