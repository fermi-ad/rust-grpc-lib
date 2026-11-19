//! Tower middleware that tracks a gRPC call through its response body.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, ready};
use std::time::Instant;

use http_body::{Body, Frame, SizeHint};
use pin_project_lite::pin_project;
use tonic::Code;
use tonic::codegen::http::{HeaderMap, Request, Response};
use tower::{Layer, Service};

use super::{
    call::{CompletionGuard, Sink},
    label::LabelIndex,
};

#[cfg(test)]
mod tests;

#[derive(Clone)]
pub(crate) struct CallLayer<T> {
    labels: Arc<LabelIndex>,
    sink: T,
}

#[derive(Clone)]
pub(crate) struct CallService<S, T> {
    inner: S,
    labels: Arc<LabelIndex>,
    sink: T,
}

pin_project! {
    pub(crate) struct CallFuture<F, T: Sink> {
        #[pin]
        inner: F,
        guard: CompletionGuard<T>,
    }
}

pin_project! {
    pub(crate) struct CallBody<B, T: Sink> {
        #[pin]
        inner: B,
        guard: CompletionGuard<T>,
    }
}

impl<T> CallLayer<T> {
    pub(crate) fn new(labels: LabelIndex, sink: T) -> Self {
        Self {
            labels: Arc::new(labels),
            sink,
        }
    }
}

impl<S, T: Sink> Layer<S> for CallLayer<T> {
    type Service = CallService<S, T>;

    fn layer(&self, inner: S) -> Self::Service {
        CallService {
            inner,
            labels: Arc::clone(&self.labels),
            sink: self.sink.clone(),
        }
    }
}

impl<S, T, Req, B> Service<Request<Req>> for CallService<S, T>
where
    S: Service<Request<Req>, Response = Response<B>>,
    B: Body,
    T: Sink,
{
    type Response = Response<CallBody<B, T>>;
    type Error = S::Error;
    type Future = CallFuture<S::Future, T>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Request<Req>) -> Self::Future {
        let start = Instant::now();
        let method = self.labels.resolve(request.uri().path());
        let guard = CompletionGuard::new(self.sink.clone(), method, start);
        CallFuture {
            inner: self.inner.call(request),
            guard,
        }
    }
}

impl<F, T, B, E> Future for CallFuture<F, T>
where
    F: Future<Output = Result<Response<B>, E>>,
    B: Body,
    T: Sink,
{
    type Output = Result<Response<CallBody<B, T>>, E>;

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

        let guard = this.guard.transfer();
        Poll::Ready(Ok(response.map(|inner| CallBody { inner, guard })))
    }
}

impl<B: Body, T: Sink> Body for CallBody<B, T> {
    type Data = B::Data;
    type Error = B::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let mut this = self.project();
        let frame = ready!(this.inner.as_mut().poll_frame(cx));
        match &frame {
            Some(Ok(frame)) => {
                if let Some(trailers) = frame.trailers_ref() {
                    this.guard
                        .finish(status_code(trailers).unwrap_or(Code::Unknown));
                } else if this.inner.is_end_stream() {
                    // A final data frame may be the last poll the transport makes.
                    this.guard.finish(Code::Unknown);
                }
            }
            Some(Err(_)) | None => this.guard.finish(Code::Unknown),
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
