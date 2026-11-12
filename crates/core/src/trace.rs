//! `GrpcServer`'s outermost layer: opens the `rpc` span per request and holds
//! it until the response body ends. Fields and target: [`crate::server`];
//! field names live in [`RpcSpan`].
//!
//! Inserts the [`RpcSpan`] and, if the path parses, the
//! [`RpcMethod`](crate::auth::RpcMethod) request extensions.
//!
//! Cancellation: dropping the response future before it yields a response
//! (client cancel or server timeout) records `grpc.status` as `Cancelled`. A
//! mid-stream cancel leaves it unset, and the span closes without `stream_ms`.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use pin_project_lite::pin_project;
use tonic::Code;
use tonic::codegen::http::Request;
use tower::{Layer, Service};
use tower_http::trace::{GrpcMakeClassifier, Trace, TraceLayer};

use crate::auth::RpcSpan;
use hooks::{LogFailure, RecordEos, RecordResponse, SpanFromExtension};

mod hooks;
#[cfg(test)]
mod tests;

/// Wraps a service in [`GrpcTrace`].
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct GrpcTraceLayer;

impl GrpcTraceLayer {
    pub(crate) fn new() -> Self {
        Self
    }
}

impl<S> Layer<S> for GrpcTraceLayer {
    type Service = GrpcTrace<S>;

    fn layer(&self, inner: S) -> Self::Service {
        // Order: `GrpcTrace` (opens the span) -> tower-http `Trace` (hooks)
        // -> `inner`.
        let inner = TraceLayer::new_for_grpc()
            .make_span_with(SpanFromExtension)
            .on_request(())
            .on_response(RecordResponse)
            .on_body_chunk(())
            .on_eos(RecordEos)
            .on_failure(LogFailure)
            .layer(inner);
        GrpcTrace { inner }
    }
}

/// Opens the span and arms a `CancelGuard` before delegating to `Trace`.
#[derive(Debug, Clone)]
pub(crate) struct GrpcTrace<S> {
    inner: InnerTrace<S>,
}

impl<S, ReqBody> Service<Request<ReqBody>> for GrpcTrace<S>
where
    InnerTrace<S>: Service<Request<ReqBody>>,
{
    type Response = <InnerTrace<S> as Service<Request<ReqBody>>>::Response;
    type Error = <InnerTrace<S> as Service<Request<ReqBody>>>::Error;
    type Future = GrpcTraceFuture<<InnerTrace<S> as Service<Request<ReqBody>>>::Future>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut request: Request<ReqBody>) -> Self::Future {
        let span = publish_extensions(&mut request);
        GrpcTraceFuture {
            inner: self.inner.call(request),
            guard: CancelGuard {
                span,
                completed: false,
            },
        }
    }
}

pin_project! {
    /// Response future of [`GrpcTrace`]; records `Cancelled` if dropped
    /// before completing.
    pub(crate) struct GrpcTraceFuture<F> {
        #[pin]
        inner: F,
        guard: CancelGuard,
    }
}

impl<F: Future> Future for GrpcTraceFuture<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        let output = ready!(this.inner.poll(cx));
        this.guard.completed = true;
        Poll::Ready(output)
    }
}

type InnerTrace<S> =
    Trace<S, GrpcMakeClassifier, SpanFromExtension, (), RecordResponse, (), RecordEos, LogFailure>;

// Covers only the response future; after it completes nothing records a
// cancel (see the module docs).
struct CancelGuard {
    span: RpcSpan,
    completed: bool,
}

impl Drop for CancelGuard {
    fn drop(&mut self) {
        if !self.completed {
            self.span.record_status(Code::Cancelled);
        }
    }
}

// Done here, not in a `Trace` hook, because hooks only get `&Request`. tonic
// does not insert `RpcMethod` server-side, so this is its only source.
fn publish_extensions<B>(request: &mut Request<B>) -> RpcSpan {
    let (span, method) = RpcSpan::open(request.uri().path());
    if let Some(method) = method {
        request.extensions_mut().insert(method);
    }
    request.extensions_mut().insert(span.clone());
    span
}
