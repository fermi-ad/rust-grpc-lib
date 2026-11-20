//! Tower middleware that tracks a gRPC call through its response body.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, ready};
use std::time::Instant;

use http_body::{Body, Frame, SizeHint};
use pin_project_lite::pin_project;
use tokio::sync::watch;
use tonic::Code;
use tonic::codegen::http::{HeaderMap, Request, Response};
use tower::{Layer, Service};

use super::{
    Telemetry,
    call::CompletionGuard,
    label::MethodLabelIndex,
    recorder::{CallCompletionObserver, CallRecorder},
};

#[cfg(test)]
mod tests;

#[derive(Clone)]
pub(crate) struct CallLayer {
    labels: Arc<MethodLabelIndex>,
    recorder: Arc<CallRecorder>,
}

#[derive(Clone)]
pub(crate) struct CallService<S> {
    inner: S,
    labels: Arc<MethodLabelIndex>,
    recorder: Arc<CallRecorder>,
}

pin_project! {
    pub(crate) struct CallFuture<F> {
        #[pin]
        inner: F,
        guard: CompletionGuard,
    }
}

pin_project! {
    pub(crate) struct CallBody<B> {
        #[pin]
        inner: B,
        guard: CompletionGuard,
    }
}

impl CallLayer {
    pub(crate) fn new(
        telemetry: &Telemetry,
        labels: MethodLabelIndex,
        on_call: Option<CallCompletionObserver>,
        call_drain_guard: watch::Receiver<()>,
    ) -> Self {
        Self {
            labels: Arc::new(labels),
            recorder: Arc::new(CallRecorder::new(telemetry, on_call, call_drain_guard)),
        }
    }
}

impl<S> Layer<S> for CallLayer {
    type Service = CallService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        CallService {
            inner,
            labels: Arc::clone(&self.labels),
            recorder: Arc::clone(&self.recorder),
        }
    }
}

impl<S, Req, B> Service<Request<Req>> for CallService<S>
where
    S: Service<Request<Req>, Response = Response<B>>,
    B: Body,
{
    type Response = Response<CallBody<B>>;
    type Error = S::Error;
    type Future = CallFuture<S::Future>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Request<Req>) -> Self::Future {
        let start = Instant::now();
        let method = self.labels.label_for_path(request.uri().path());
        let guard = CompletionGuard::new(Arc::clone(&self.recorder), method, start);
        CallFuture {
            inner: self.inner.call(request),
            guard,
        }
    }
}

impl<F, B, E> Future for CallFuture<F>
where
    F: Future<Output = Result<Response<B>, E>>,
    B: Body,
{
    type Output = Result<Response<CallBody<B>>, E>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        let response = match ready!(this.inner.poll(cx)) {
            Ok(response) => response,
            Err(error) => {
                this.guard.finish(Code::Unknown);
                return Poll::Ready(Err(error));
            }
        };

        if let Some(status) = status_code(response.headers()) {
            this.guard.finish(status);
        } else if response.body().is_end_stream() {
            // The transport need not poll a body that is already exhausted.
            this.guard.finish(Code::Unknown);
        }

        let guard = this.guard.take_completion();
        Poll::Ready(Ok(response.map(|inner| CallBody { inner, guard })))
    }
}

impl<B: Body> Body for CallBody<B> {
    type Data = B::Data;
    type Error = B::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let mut this = self.project();
        let frame = ready!(this.inner.as_mut().poll_frame(cx));
        let status = match &frame {
            Some(Ok(frame)) => {
                if let Some(trailers) = frame.trailers_ref() {
                    Some(status_code(trailers).unwrap_or(Code::Unknown))
                } else {
                    // A final data frame may be the last poll the transport makes.
                    this.inner.is_end_stream().then_some(Code::Unknown)
                }
            }
            Some(Err(_)) | None => Some(Code::Unknown),
        };
        if let Some(status) = status {
            this.guard.finish(status);
        }
        Poll::Ready(frame)
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

// Same parsing rule as trace/hooks.rs; the trace hooks remain independent.
fn status_code(headers: &HeaderMap) -> Option<Code> {
    headers
        .get("grpc-status")
        .map(|value| Code::from_bytes(value.as_bytes()))
}
