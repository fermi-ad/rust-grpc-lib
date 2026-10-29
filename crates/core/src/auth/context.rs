//! A per-request bundle of the message, the validated claims, and the request
//! envelope (metadata, extensions, peer address, RPC method).
//!
//! [`RequestContext`] is the supported way for a handler to read the claims
//! that [`JwtValidationService`](crate::auth::layer::JwtValidationService)
//! inserted into the request extensions. It replaces the hand-written
//! `request.extensions().get::<KeycloakClaims>()` prologue:
//!
//! ```rust,ignore
//! async fn set_armed_state(
//!     &self,
//!     request: Request<SetArmedStateRequest>,
//! ) -> Result<Response<SetArmedStateResponse>, Status> {
//!     let ctx = RequestContext::try_from_request(request)?;
//!     if !ctx.has_role("operator") {
//!         return Err(Status::permission_denied("operator role required"));
//!     }
//!     let who = ctx.sub();
//!     let message = ctx.message;
//!     // ...
//! }
//! ```
//!
//! # Failure mode
//!
//! If the claims extension is absent, the server was wired without the JWT
//! validation layer or interceptor. That is a server-side misconfiguration,
//! not a client error, so [`RequestContext::try_from_request`] fails closed
//! with [`Status::internal`] and a generic message. The detail (including the
//! expected claims type) is logged server-side with `tracing::error!` when the
//! `tracing` feature is enabled, and is never sent to the remote client.

use std::any::type_name;
use std::net::SocketAddr;

use rust_auth_lib::{Claims, keycloak::KeycloakClaims};
use tonic::{Extensions, Request, Status, metadata::MetadataMap};

#[cfg(test)]
mod tests;

/// Everything a handler needs to know about an authenticated request.
///
/// Construct it with [`RequestContext::try_from_request`]. `T` is the message
/// type: the request message for unary RPCs, or `tonic::Streaming<Req>` for
/// client- and bidirectional-streaming RPCs. `C` is the claims type and
/// defaults to [`KeycloakClaims`].
///
/// The claims are *cloned* out of the request extensions, so
/// [`extensions`](Self::extensions) still contains them.
#[derive(Debug)]
pub struct RequestContext<T, C = KeycloakClaims> {
    /// The validated claims inserted by the JWT validation layer.
    pub claims: C,
    /// The request message (or request stream).
    pub message: T,
    /// The request's gRPC metadata (headers).
    pub metadata: MetadataMap,
    /// The request extensions as they were when the handler was called.
    pub extensions: Extensions,
    /// The peer's socket address, when the transport reports one.
    pub remote_addr: Option<SocketAddr>,
    /// The RPC being served, when an [`RpcMethod`] extension was inserted by
    /// an outer layer; `None` otherwise.
    pub rpc_method: Option<RpcMethod>,
}

impl<T, C> RequestContext<T, C> {
    /// Build a context from an incoming request.
    ///
    /// The remote address is read before the request is taken apart, because
    /// `Request::remote_addr` reads the connection info from the extensions.
    /// Works for streaming requests (`T = tonic::Streaming<Req>`).
    ///
    /// # Errors
    ///
    /// Returns [`Status::internal`] with the generic message
    /// `"authentication is not configured"` when no claims of type `C` are in
    /// the request extensions. See the [module docs](self).
    pub fn try_from_request(request: Request<T>) -> Result<Self, Status>
    where
        C: Clone + Send + Sync + 'static,
    {
        let remote_addr = request.remote_addr();
        let (metadata, extensions, message) = request.into_parts();

        let claims = match extensions.get::<C>() {
            Some(claims) => claims.clone(),
            None => {
                #[cfg(feature = "tracing")]
                tracing::error!(
                    claims_type = type_name::<C>(),
                    "request has no claims extension; the JWT validation layer or \
                     interceptor is not installed for this service"
                );
                return Err(Status::internal("authentication is not configured"));
            }
        };

        let rpc_method = extensions.get::<RpcMethod>().cloned();

        Ok(Self {
            claims,
            message,
            metadata,
            extensions,
            remote_addr,
            rpc_method,
        })
    }

    /// The token subject (`sub` claim).
    pub fn sub(&self) -> &str
    where
        C: Claims,
    {
        self.claims.sub()
    }

    /// The token expiry (`exp` claim) as seconds since the Unix epoch.
    ///
    /// Long-lived stream handlers can compare this with the current time to
    /// decide when to end the stream.
    pub fn exp_unix(&self) -> u64
    where
        C: Claims,
    {
        self.claims.exp_unix()
    }

    /// Whether the token has expired, evaluated at call time.
    ///
    /// The validation layer rejects expired tokens when a request starts, but a
    /// stream can outlive its token, so stream handlers should re-check this.
    pub fn is_expired(&self) -> bool
    where
        C: Claims,
    {
        self.claims.is_expired()
    }
}

// A second inherent `impl` is unavoidable here: role data only exists on
// `KeycloakClaims`, and a method-level `where` clause cannot pin `C` to a
// concrete type.
impl<T> RequestContext<T, KeycloakClaims> {
    /// Whether the caller has the given realm role.
    pub fn has_role(&self, role: &str) -> bool {
        self.claims.has_role(role)
    }
}

/// The gRPC method an RPC was dispatched to, parsed from the request path
/// `/<service>/<method>` (for example `/services.example.v1.ExampleDB/GetEndPointOne`).
///
/// tonic does not insert this on the server side, so it is populated by a
/// library-owned layer that is not part of this release. Until that layer is
/// installed, [`RequestContext::rpc_method`] is `None`. Anything that inserts
/// an `RpcMethod` into the request extensions will be picked up by
/// [`RequestContext::try_from_request`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RpcMethod {
    /// Fully qualified service name, e.g. `services.example.v1.ExampleDB`.
    pub service: String,
    /// Method name, e.g. `GetEndPointOne`.
    pub method: String,
}
