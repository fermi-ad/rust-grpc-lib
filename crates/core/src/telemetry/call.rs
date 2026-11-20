//! Per-call state and the shared terminal notification.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use tonic::Code;

use super::recorder::CallRecorder;

#[cfg(test)]
mod tests;

/// What [`GrpcServerBuilder::on_call_complete`](crate::server::GrpcServerBuilder::on_call_complete)
/// observers receive for each finished call.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct CallSummary {
    /// Canonical `package.Service/Method`, or `None` for an unlisted path.
    pub method: Option<&'static str>,
    /// The final gRPC status, or the status inferred from termination.
    pub status: Code,
    /// From request receipt to final status or drop.
    pub duration: Duration,
}

/// The response future owns this until it can hand it to the response body.
/// Whichever path consumes the state first owns the terminal notification.
pub(super) struct CompletionGuard {
    recorder: Option<Arc<CallRecorder>>,
    method: Option<&'static str>,
    start: Instant,
}

impl CompletionGuard {
    pub(super) fn new(
        recorder: Arc<CallRecorder>,
        method: Option<&'static str>,
        start: Instant,
    ) -> Self {
        recorder.started(method);
        Self {
            recorder: Some(recorder),
            method,
            start,
        }
    }

    pub(super) fn take_completion(&mut self) -> Self {
        Self {
            recorder: self.recorder.take(),
            method: self.method,
            start: self.start,
        }
    }

    pub(super) fn finish(&mut self, status: Code) {
        if let Some(recorder) = self.recorder.take() {
            recorder.finished(CallSummary {
                method: self.method,
                status,
                duration: self.start.elapsed(),
            });
        }
    }
}

impl Drop for CompletionGuard {
    fn drop(&mut self) {
        self.finish(Code::Cancelled);
    }
}
