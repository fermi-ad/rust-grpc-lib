//! [`CallInfo`]: the call details shared by both handler contexts.

use std::future::Future;
use std::net::SocketAddr;

use tonic::{Extensions, Request, Status, metadata::MetadataMap};
use tracing::{Instrument, Span, instrument::Instrumented};

use super::{RpcMethod, RpcSpan};

#[cfg(test)]
mod tests;

/// The details of one call, shared by
/// [`RequestContext`](super::RequestContext) and
/// [`PublicContext`](super::PublicContext) through `Deref` (read-only: a
/// handler cannot change them). Helpers that only log or trace can take a
/// `&CallInfo`.
///
/// [`metadata`](Self::metadata) never contains the `authorization` header.
#[derive(Debug)]
pub struct CallInfo {
    /// The RPC being served.
    pub rpc: RpcMethod,
    /// The peer's socket address, when the transport reports one.
    pub remote_addr: Option<SocketAddr>,
    /// The request's gRPC metadata (headers), without `authorization`.
    pub metadata: MetadataMap,
    /// The request extensions.
    pub extensions: Extensions,
    span: RpcSpan,
}

impl CallInfo {
    /// Split a request into the call details and the message. A missing
    /// [`RpcSpan`] falls back to [`RpcSpan::none`].
    ///
    /// # Errors
    ///
    /// [`not_configured`] when there is no [`RpcMethod`] (the gate never saw
    /// the request).
    pub(super) fn from_request<T>(request: Request<T>) -> Result<(Self, T), Status> {
        // Read first: `Request::remote_addr` needs the extensions that
        // `into_parts` takes apart.
        let remote_addr = request.remote_addr();
        let (metadata, extensions, message) = request.into_parts();
        let Some(rpc) = extensions.get::<RpcMethod>().cloned() else {
            return Err(not_configured(
                "request has no RpcMethod extension; the service is not served through \
                 `GrpcServer::add_service`",
            ));
        };
        let span = extensions
            .get::<RpcSpan>()
            .cloned()
            .unwrap_or_else(RpcSpan::none);
        let info = Self {
            rpc,
            remote_addr,
            metadata,
            extensions,
            span,
        };
        Ok((info, message))
    }

    // The details of a call in a unit test: the method `test.Service/Test`,
    // no peer, empty metadata and extensions, and a span that records nothing.
    #[cfg(any(feature = "test-util", test))]
    pub(super) fn for_test() -> Self {
        Self {
            rpc: RpcMethod::for_test(),
            remote_addr: None,
            metadata: MetadataMap::new(),
            extensions: Extensions::new(),
            span: RpcSpan::none(),
        }
    }

    /// The `rpc` span of this call ([`Span::none`] without the trace layer).
    /// It is already current in the handler; use this for work that leaves
    /// it, or see [`instrument`](Self::instrument).
    #[must_use = "this only borrows the span; use it to enter, record onto, or parent another span"]
    pub fn span(&self) -> &Span {
        self.span.span()
    }

    /// Run `future` inside the `rpc` span, e.g. a spawned task or a
    /// streaming response body:
    ///
    /// ```rust
    /// # use std::sync::mpsc::Sender;
    /// # use rust_grpc_lib::auth::RequestContext;
    /// # async fn produce(_tx: Sender<u32>) {}
    /// # fn handler(ctx: &RequestContext<()>, tx: Sender<u32>) {
    /// tokio::spawn(ctx.instrument(async move { produce(tx).await }));
    /// # }
    /// ```
    ///
    /// If `tracing::Instrument` is in scope, `ctx.instrument(..)` resolves to
    /// that trait instead; write `future.instrument(ctx.span().clone())`.
    #[must_use = "a future does nothing unless it is awaited or spawned"]
    pub fn instrument<F: Future>(&self, future: F) -> Instrumented<F> {
        future.instrument(self.span().clone())
    }
}

// A server misconfiguration, not a client error: `detail` is logged and never
// sent, so a probe learns nothing about the wiring.
pub(super) fn not_configured(detail: &str) -> Status {
    tracing::error!("{detail}");
    Status::internal("authentication is not configured")
}
