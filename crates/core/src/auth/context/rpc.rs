//! The per-RPC request extensions: [`RpcMethod`] and [`RpcSpan`].
//!
//! Both are inserted by the `GrpcTraceLayer` (`trace` feature) and read back
//! by [`RequestContext::try_from_request`](super::RequestContext::try_from_request).

#[cfg(any(feature = "trace", test))]
use std::{fmt::Display, time::Duration};

#[cfg(any(feature = "trace", test))]
use tonic::Code;
use tracing::Span;

#[cfg(test)]
mod tests;

/// The gRPC method an RPC was dispatched to, parsed from the request path
/// `/<service>/<method>` (for example `/services.example.v1.ExampleDB/GetEndPointOne`).
///
/// tonic does not insert this on the server side, so it is populated by the
/// `GrpcTraceLayer` (`trace` feature). Until that layer is installed,
/// [`RequestContext::rpc_method`](super::RequestContext::rpc_method) is `None`.
/// Anything that inserts an `RpcMethod` into the request extensions will be
/// picked up by
/// [`RequestContext::try_from_request`](super::RequestContext::try_from_request).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RpcMethod {
    /// Fully qualified service name, e.g. `services.example.v1.ExampleDB`.
    pub service: String,
    /// Method name, e.g. `GetEndPointOne`.
    pub method: String,
}

impl RpcMethod {
    /// Parse a gRPC request path of the form `/<service>/<method>`.
    ///
    /// Returns `None` when the path does not have exactly those two non-empty
    /// segments.
    pub fn from_path(path: &str) -> Option<Self> {
        let (service, method) = path.strip_prefix('/')?.split_once('/')?;
        if service.is_empty() || method.is_empty() || method.contains('/') {
            return None;
        }
        Some(Self {
            service: service.to_owned(),
            method: method.to_owned(),
        })
    }
}

/// The `tracing` span that covers one RPC, inserted into the request
/// extensions by the `GrpcTraceLayer` (`trace` feature).
///
/// [`RequestContext::try_from_request`](super::RequestContext::try_from_request)
/// records the caller's `sub` claim onto this span as its `user` field. It
/// records onto this span directly instead of `Span::current()`, because a
/// handler-level `#[instrument]` becomes the current span and `Span::record`
/// silently ignores fields the span did not declare.
///
/// This type is the only place that knows the span's field names: they are
/// declared in `RpcSpan::open` and recorded through the `record_*` methods,
/// so a renamed field cannot be silently missed by one of its writers.
#[derive(Debug, Clone)]
pub struct RpcSpan(Span);

impl RpcSpan {
    /// Open the `rpc` span for a request path.
    ///
    /// Returns the span together with the [`RpcMethod`] parsed from the path
    /// (`None`, and `unknown` service and method fields, when it does not
    /// parse). The span's target is `rust_grpc_lib::trace`, so it can be
    /// filtered as a unit. Only `GrpcTraceLayer` uses this.
    #[cfg(any(feature = "trace", test))]
    pub(crate) fn open(path: &str) -> (Self, Option<RpcMethod>) {
        let method = RpcMethod::from_path(path);
        let (service, method_name) = match &method {
            Some(method) => (method.service.as_str(), method.method.as_str()),
            None => ("unknown", "unknown"),
        };
        let span = tracing::info_span!(
            target: "rust_grpc_lib::trace",
            "rpc",
            rpc.system = "grpc",
            rpc.service = service,
            rpc.method = method_name,
            grpc.status = tracing::field::Empty,
            latency_ms = tracing::field::Empty,
            stream_ms = tracing::field::Empty,
            user = tracing::field::Empty,
        );
        (Self(span), method)
    }

    /// View a span that [`open`](Self::open) created, for the tower-http
    /// hooks, which are handed a bare `&Span`.
    #[cfg(any(feature = "trace", test))]
    pub(crate) fn wrap(span: &Span) -> Self {
        Self(span.clone())
    }

    /// The span that covers the RPC.
    pub fn span(&self) -> &Span {
        &self.0
    }

    pub(crate) fn record_user(&self, sub: &str) {
        self.0.record("user", sub);
    }

    #[cfg(any(feature = "trace", test))]
    pub(crate) fn record_status(&self, code: Code) {
        self.0.record("grpc.status", tracing::field::debug(code));
    }

    #[cfg(any(feature = "trace", test))]
    pub(crate) fn record_latency(&self, latency: Duration) {
        self.0.record("latency_ms", millis(latency));
    }

    #[cfg(any(feature = "trace", test))]
    pub(crate) fn record_stream(&self, duration: Duration) {
        self.0.record("stream_ms", millis(duration));
    }

    /// Log a transport or body failure as a `warn` event inside the span.
    #[cfg(any(feature = "trace", test))]
    pub(crate) fn record_failure(&self, error: impl Display, latency: Duration) {
        tracing::warn!(
            target: "rust_grpc_lib::trace",
            parent: &self.0,
            %error,
            latency_ms = millis(latency),
            "rpc failed"
        );
    }
}

#[cfg(any(feature = "trace", test))]
fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}
