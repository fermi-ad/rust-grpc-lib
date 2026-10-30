//! Per-RPC `tracing` spans for a tonic server.
//!
//! [`GrpcTraceLayer`] opens one `rpc` span per request and keeps it open
//! until the response stream ends, so unary and streaming RPCs are both
//! covered. It also publishes two request extensions that handlers can read:
//!
//! - [`RpcMethod`](crate::auth::RpcMethod), parsed from the request path. tonic
//!   does not insert one on the server side, so this layer is what fills
//!   [`RequestContext::rpc_method`](crate::auth::RequestContext::rpc_method).
//! - [`RpcSpan`](crate::auth::RpcSpan), the span itself.
//!   [`RequestContext::try_from_request`](crate::auth::RequestContext::try_from_request)
//!   records the caller's `sub` onto it as `user`. See [`RpcSpan`](crate::auth::RpcSpan)
//!   for why it does not use `Span::current()`.
//!
//! Enabled by the `trace` feature (off by default).
//!
//! # Span fields
//!
//! | Field | Value |
//! |---|---|
//! | `rpc.system` | always `"grpc"` |
//! | `rpc.service` | e.g. `services.example.ExampleDB`, or `unknown` for an unparseable path |
//! | `rpc.method` | e.g. `getEndPointOne`, or `unknown` |
//! | `grpc.status` | the status code name (`Ok`, `Unauthenticated`, ...), recorded when known |
//! | `latency_ms` | time until the response headers were produced; for a unary RPC, the handler time |
//! | `stream_ms` | how long the response stream lasted after that |
//! | `user` | the token subject, recorded by `RequestContext::try_from_request` |
//!
//! The span's target is `rust_grpc_lib::trace`, so the spans (and the `warn`
//! event logged for transport errors) can be filtered as a unit.
//!
//! # Cancellation
//!
//! If the response future is dropped before it produces a response (the
//! client cancelled, or tonic's timeout fired), `grpc.status` is recorded as
//! `Cancelled`. tonic does not say which of the two happened.
//!
//! Once the response headers exist, the future has completed and this no
//! longer applies. If the client cancels *mid-stream* (the response body is
//! dropped before its trailers), `grpc.status` is left unset unless the
//! status was already known from the headers. The span closes with
//! `latency_ms` but without `stream_ms`.
//!
//! # Placement
//!
//! Install the layer on tonic's `Server` before adding services, and **before
//! any other layer**, so it is outermost and also sees requests that an
//! authentication layer rejects:
//!
//! ```rust,ignore
//! Server::builder()
//!     .layer(GrpcTraceLayer::new())
//!     .add_service(health)
//!     .add_service(wrap_service::<KeycloakClaims, _, _>(MyServiceServer::new(my), validator))
//! ```
//!
//! `Server::layer` wraps every service, health included, so health probes get
//! a span too; filter the `rust_grpc_lib::trace` target if they are noise. The
//! layer does not authenticate anything.

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

type InnerTrace<S> =
    Trace<S, GrpcMakeClassifier, SpanFromExtension, (), RecordResponse, (), RecordEos, LogFailure>;

/// A tower [`Layer`] that opens an `rpc` span per gRPC request.
///
/// See the [module docs](self) for the fields, the extensions it inserts and
/// where to install it.
#[derive(Debug, Clone, Copy, Default)]
pub struct GrpcTraceLayer;

impl GrpcTraceLayer {
    /// Create the layer.
    pub fn new() -> Self {
        Self
    }
}

impl<S> Layer<S> for GrpcTraceLayer {
    type Service = GrpcTrace<S>;

    fn layer(&self, inner: S) -> Self::Service {
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

/// The service produced by [`GrpcTraceLayer`].
#[derive(Debug, Clone)]
pub struct GrpcTrace<S> {
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
    /// The response future of [`GrpcTrace`].
    ///
    /// If it is dropped before it completes, the RPC's `grpc.status` is
    /// recorded as `Cancelled`.
    pub struct GrpcTraceFuture<F> {
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

/// Records `Cancelled` on the span if the future is dropped before it
/// completes.
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

/// Open the `rpc` span for `request` and publish the `RpcMethod` and
/// `RpcSpan` extensions. Returns the span.
fn publish_extensions<B>(request: &mut Request<B>) -> RpcSpan {
    let (span, method) = RpcSpan::open(request.uri().path());
    if let Some(method) = method {
        request.extensions_mut().insert(method);
    }
    request.extensions_mut().insert(span.clone());
    span
}
