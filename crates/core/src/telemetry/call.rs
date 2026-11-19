//! Per-call state and the shared terminal notification.

use std::time::{Duration, Instant};

use tonic::Code;

#[cfg(test)]
mod tests;

/// The outcome of one finished gRPC call.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct CallSummary {
    /// Canonical `package.Service/Method`, or `None` for an unlisted path.
    pub method: Option<&'static str>,
    /// The final gRPC status, or the status inferred from termination.
    pub status: Code,
    /// Time from request receipt to final status or drop.
    pub duration: Duration,
}

pub(crate) trait Sink: Clone {
    fn started(&self, method: Option<&'static str>);
    fn finished(&self, summary: CallSummary);
}

/// The response future owns this until it can hand it to the response body.
/// Whichever path consumes the state first owns the terminal notification.
pub(super) struct CompletionGuard<T: Sink> {
    state: Option<State<T>>,
}

struct State<T> {
    sink: T,
    method: Option<&'static str>,
    start: Instant,
}

impl<T: Sink> CompletionGuard<T> {
    pub(super) fn new(sink: T, method: Option<&'static str>, start: Instant) -> Self {
        sink.started(method);
        Self {
            state: Some(State {
                sink,
                method,
                start,
            }),
        }
    }

    pub(super) fn transfer(&mut self) -> Self {
        Self {
            state: self.state.take(),
        }
    }

    pub(super) fn finish(&mut self, status: Code) {
        if let Some(State {
            sink,
            method,
            start,
        }) = self.state.take()
        {
            sink.finished(CallSummary {
                method,
                status,
                duration: start.elapsed(),
            });
        }
    }
}

impl<T: Sink> Drop for CompletionGuard<T> {
    fn drop(&mut self) {
        self.finish(Code::Cancelled);
    }
}
