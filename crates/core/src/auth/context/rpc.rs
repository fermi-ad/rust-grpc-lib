//! The per-RPC request extensions [`RpcMethod`] and [`RpcSpan`], both
//! inserted by the trace layer. The gate re-checks `RpcMethod` against the
//! path and records onto the span.

use std::fmt::Display;
use std::time::Duration;

use tonic::Code;
use tracing::Span;

#[cfg(test)]
mod tests;

/// The gRPC method being called, parsed from the path `/<service>/<method>`.
/// Handlers read it as [`CallInfo::rpc`](super::CallInfo::rpc). Names are
/// the proto wire names (`GetEndPointOne`), not Rust names.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RpcMethod {
    /// Fully qualified service name, e.g. `services.example.v1.ExampleDB`.
    pub service: String,
    /// Method name, e.g. `GetEndPointOne`.
    pub method: String,
}

impl RpcMethod {
    // Parse `/<service>/<method>`. `None` unless there are exactly two
    // non-empty segments, so the gate denies anything else.
    pub(crate) fn from_path(path: &str) -> Option<Self> {
        Self::from_path_reusing(path, None)
    }

    // Like `from_path`, but reuses `known` instead of allocating when it
    // names the same method.
    //
    // Security: the path is always parsed and `known` is only trusted when it
    // agrees, so a stale or forged extension cannot change the gate's decision.
    pub(crate) fn from_path_reusing(path: &str, known: Option<Self>) -> Option<Self> {
        let (service, method) = path.strip_prefix('/')?.split_once('/')?;
        if service.is_empty() || method.is_empty() || method.contains('/') {
            return None;
        }
        match known {
            Some(known) if known.service == service && known.method == method => Some(known),
            _ => Some(Self {
                service: service.to_owned(),
                method: method.to_owned(),
            }),
        }
    }

    // The method reported by contexts built with `for_test`:
    // `test.Service/Test`.
    #[cfg(any(feature = "test-util", test))]
    pub(crate) fn for_test() -> Self {
        Self {
            service: "test.Service".to_owned(),
            method: "Test".to_owned(),
        }
    }
}

// The `rpc` span of one call; handlers reach it through `CallInfo::span`.
//
// Writers record onto this span, not `Span::current()`: a handler's
// `#[instrument]` span may be current, and `Span::record` silently ignores
// undeclared fields. Field names live only here (`open` and `record_*`), so
// a rename cannot miss a writer.
#[derive(Debug, Clone)]
pub(crate) struct RpcSpan(Span);

impl RpcSpan {
    // Open the `rpc` span (target `rust_grpc_lib::trace`) for a path, with the
    // parsed `RpcMethod` (`None`, and `unknown` fields, if it does not parse).
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
            access = tracing::field::Empty,
        );
        (Self(span), method)
    }

    // A span that records nothing, for calls without the trace layer.
    pub(crate) fn none() -> Self {
        Self(Span::none())
    }

    // Re-wrap a span from `open` for the tower-http hooks, which get `&Span`.
    pub(crate) fn wrap(span: &Span) -> Self {
        Self(span.clone())
    }

    pub(crate) fn span(&self) -> &Span {
        &self.0
    }

    pub(crate) fn record_user(&self, sub: &str) {
        self.0.record("user", sub);
    }

    // `Access::name` of the called method.
    pub(crate) fn record_access(&self, access: &str) {
        self.0.record("access", access);
    }

    pub(crate) fn record_status(&self, code: Code) {
        self.0.record("grpc.status", tracing::field::debug(code));
    }

    pub(crate) fn record_latency(&self, latency: Duration) {
        self.0.record("latency_ms", millis(latency));
    }

    pub(crate) fn record_stream(&self, duration: Duration) {
        self.0.record("stream_ms", millis(duration));
    }

    // Log a transport or body failure as a `warn` event inside the span.
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

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}
