//! Built-in server call metrics and completion observer.

use std::{
    collections::HashMap,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Arc,
};

use opentelemetry::{
    KeyValue,
    metrics::{Histogram, UpDownCounter},
};
use tokio::sync::watch;
use tonic::Code;

use super::{CallSummary, LOG_TARGET, Telemetry, call::Sink, label::LabelIndex, layer::CallLayer};

#[cfg(test)]
mod tests;

/// Runs once at call completion, on the serving path. Keep it cheap and
/// non-blocking. Panics are caught and logged after the metrics are recorded.
pub(crate) type OnCall = Arc<dyn Fn(&CallSummary) + Send + Sync>;

#[derive(Clone)]
pub(crate) struct Recorder {
    inner: Arc<RecorderInner>,
}

struct RecorderInner {
    duration: Histogram<f64>,
    active: UpDownCounter<i64>,
    methods: HashMap<&'static str, [KeyValue; 2]>,
    other: [KeyValue; 2],
    on_call: Option<OnCall>,
    // Shared by the layer, each service and all call completion guards.
    // Dropped only after the last guard records its outcome.
    _serving: watch::Receiver<()>,
}

impl CallLayer<Recorder> {
    pub(crate) fn recording(
        telemetry: &Telemetry,
        labels: LabelIndex,
        on_call: Option<OnCall>,
        serving: watch::Receiver<()>,
    ) -> Self {
        let recorder = Recorder::new(telemetry, &labels, on_call, serving);
        Self::new(labels, recorder)
    }
}

impl Recorder {
    fn new(
        telemetry: &Telemetry,
        labels: &LabelIndex,
        on_call: Option<OnCall>,
        serving: watch::Receiver<()>,
    ) -> Self {
        Self {
            inner: Arc::new(RecorderInner {
                duration: telemetry.duration_histogram(
                    "rpc.server.call.duration",
                    "Duration of incoming gRPC calls",
                ),
                active: telemetry
                    .meter()
                    .i64_up_down_counter("rpc.server.active_requests")
                    .with_description("Number of in-flight gRPC calls")
                    .with_unit("{request}")
                    .build(),
                methods: labels
                    .methods()
                    .map(|label| (label, method_attributes(label)))
                    .collect(),
                other: method_attributes("_OTHER"),
                on_call,
                _serving: serving,
            }),
        }
    }

    fn method_attributes(&self, method: Option<&'static str>) -> &[KeyValue; 2] {
        method
            .and_then(|label| self.inner.methods.get(label))
            .unwrap_or(&self.inner.other)
    }
}

impl Sink for Recorder {
    fn started(&self, method: Option<&'static str>) {
        self.inner.active.add(1, self.method_attributes(method));
    }

    fn finished(&self, summary: CallSummary) {
        let method = self.method_attributes(summary.method);
        self.inner.active.add(-1, method);

        let [system, method] = method.clone();
        let status = KeyValue::new("rpc.response.status_code", summary.status as i64);
        let seconds = summary.duration.as_secs_f64();
        if let Some(name) = error_name(summary.status) {
            self.inner.duration.record(
                seconds,
                &[system, method, status, KeyValue::new("error.type", name)],
            );
        } else {
            self.inner
                .duration
                .record(seconds, &[system, method, status]);
        }

        if let Some(on_call) = &self.inner.on_call
            && let Err(payload) = catch_unwind(AssertUnwindSafe(|| on_call(&summary)))
        {
            tracing::error!(target: LOG_TARGET, "gRPC call observer panicked");
            // Even dropping a custom panic payload can panic. Don't let it
            // unwind through the response path (or a cancellation guard).
            if let Err(payload) = catch_unwind(AssertUnwindSafe(|| drop(payload))) {
                std::mem::forget(payload);
            }
        }
    }
}

fn method_attributes(label: &'static str) -> [KeyValue; 2] {
    [
        KeyValue::new("rpc.system.name", "grpc"),
        KeyValue::new("rpc.method", label),
    ]
}

fn error_name(code: Code) -> Option<&'static str> {
    Some(match code {
        Code::Ok => return None,
        Code::Cancelled => "Cancelled",
        Code::Unknown => "Unknown",
        Code::InvalidArgument => "InvalidArgument",
        Code::DeadlineExceeded => "DeadlineExceeded",
        Code::NotFound => "NotFound",
        Code::AlreadyExists => "AlreadyExists",
        Code::PermissionDenied => "PermissionDenied",
        Code::ResourceExhausted => "ResourceExhausted",
        Code::FailedPrecondition => "FailedPrecondition",
        Code::Aborted => "Aborted",
        Code::OutOfRange => "OutOfRange",
        Code::Unimplemented => "Unimplemented",
        Code::Internal => "Internal",
        Code::Unavailable => "Unavailable",
        Code::DataLoss => "DataLoss",
        Code::Unauthenticated => "Unauthenticated",
    })
}
