//! The hooks that plug `GrpcTraceLayer`'s span into tower-http's `Trace`.
//!
//! The span itself is created by `GrpcTraceLayer` before `Trace` runs, because
//! `Trace` only hands its hooks a shared `&Request` and so cannot insert the
//! `RpcMethod` and `RpcSpan` extensions. [`SpanFromExtension`] therefore only
//! looks the span up. The other hooks record the outcome onto it, through
//! [`RpcSpan`], which owns the field names.
//!
//! The types are `pub` so they can appear in the bounds of the public
//! service, but this module is private and they cannot be named by consumers.

use std::time::Duration;

use tonic::Code;
use tonic::codegen::http::{HeaderMap, Request, Response};
use tower_http::classify::GrpcFailureClass;
use tower_http::trace::{MakeSpan, OnEos, OnFailure, OnResponse};
use tracing::Span;

use crate::auth::RpcSpan;

/// Uses the span that `GrpcTraceLayer` stored in the request extensions.
#[derive(Debug, Clone, Copy, Default)]
pub struct SpanFromExtension;

/// Records the time to the response and, for trailers-only responses (every
/// error status), the `grpc-status`.
#[derive(Debug, Clone, Copy, Default)]
pub struct RecordResponse;

/// Records how long the response stream lasted and the `grpc-status` carried
/// in its trailers (the normal case for a successful RPC).
#[derive(Debug, Clone, Copy, Default)]
pub struct RecordEos;

/// Logs transport and body errors. A non-zero `grpc-status` is not logged
/// here: it is already a field of the span.
#[derive(Debug, Clone, Copy, Default)]
pub struct LogFailure;

impl<B> MakeSpan<B> for SpanFromExtension {
    fn make_span(&mut self, request: &Request<B>) -> Span {
        let span = request
            .extensions()
            .get::<RpcSpan>()
            .map(|rpc_span| rpc_span.span().clone());
        debug_assert!(
            span.is_some(),
            "GrpcTraceLayer inserts the RpcSpan before the inner Trace service runs"
        );
        span.unwrap_or_else(Span::none)
    }
}

impl<B> OnResponse<B> for RecordResponse {
    fn on_response(self, response: &Response<B>, latency: Duration, span: &Span) {
        let span = RpcSpan::wrap(span);
        span.record_latency(latency);
        if let Some(code) = status_code(response.headers()) {
            span.record_status(code);
        }
    }
}

impl OnEos for RecordEos {
    fn on_eos(self, trailers: Option<&HeaderMap>, stream_duration: Duration, span: &Span) {
        let span = RpcSpan::wrap(span);
        span.record_stream(stream_duration);
        if let Some(code) = trailers.and_then(status_code) {
            span.record_status(code);
        }
    }
}

impl OnFailure<GrpcFailureClass> for LogFailure {
    fn on_failure(&mut self, failure: GrpcFailureClass, latency: Duration, span: &Span) {
        if let GrpcFailureClass::Error(error) = failure {
            RpcSpan::wrap(span).record_failure(error, latency);
        }
    }
}

fn status_code(headers: &HeaderMap) -> Option<Code> {
    headers
        .get("grpc-status")
        .map(|value| Code::from_bytes(value.as_bytes()))
}
