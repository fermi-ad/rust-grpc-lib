//! Handler parameters: [`RequestContext`], [`PublicContext`] and the
//! [`CallInfo`] they dereference to.

// The macro rewrites the context parameter to `tonic::Request<T>` (to match
// the generated trait) and rebuilds the context with `try_from_request`,
// from the extensions that the trace layer and the gate inserted.

use std::ops::Deref;

use rust_auth_lib::{Claims, Ephemeral, ForwardedToken, TokenProvider, keycloak::KeycloakClaims};
use tonic::{Request, Status};

use super::caller::{Caller, Redacted};
use super::interceptor::bearer_header;
use call::not_configured;

mod call;
mod public;
mod rpc;
#[cfg(test)]
mod tests;

pub use call::CallInfo;
pub use public::PublicContext;
pub use rpc::RpcMethod;
pub(crate) use rpc::RpcSpan;

/// The handler parameter of a `#[roles(..)]` or `#[any_authenticated]`
/// method: the validated claims, the message and the call details. The gate
/// has validated the token and checked the roles before the handler runs.
/// `#[public]` methods take a [`PublicContext`] instead.
///
/// ```rust
/// # use std::marker::PhantomData;
/// # use rust_grpc_lib::access_controlled_service;
/// # use rust_grpc_lib::auth::{PublicContext, RequestContext};
/// # use rust_grpc_lib::tonic::{Request, Response, Status};
/// # struct Arm; struct Ping; struct Reply;
/// # trait Arming {
/// #     async fn arm(&self, r: Request<Arm>) -> Result<Response<Reply>, Status>;
/// #     async fn ping(&self, r: Request<Ping>) -> Result<Response<Reply>, Status>;
/// # }
/// # struct ArmingServer<T>(PhantomData<T>);
/// # struct MyService;
/// #[access_controlled_service]
/// impl Arming for MyService {
///     #[roles(any("operator"))]
///     async fn arm(&self, ctx: RequestContext<Arm>) -> Result<Response<Reply>, Status> {
///         tracing::info!(who = ctx.sub(), "arming");
///         # Ok(Response::new(Reply))
///     }
///
///     #[public]
///     async fn ping(&self, ctx: PublicContext<Ping>) -> Result<Response<Reply>, Status> {
///         tracing::info!(method = %ctx.rpc.method, "ping");
///         # Ok(Response::new(Reply))
///     }
/// }
/// # fn main() {}
/// ```
///
/// `T` is the message, or `tonic::Streaming<Req>` for client- and
/// bidirectional-streaming RPCs. It dereferences (read-only) to [`CallInfo`]
/// (`ctx.rpc`, `ctx.metadata`, `ctx.span()`, ...), so a handler cannot change
/// the call details. The token is not in the metadata, and `Debug` prints it
/// as `<redacted>`.
#[derive(Debug)]
pub struct RequestContext<T> {
    /// The validated claims of the caller.
    pub claims: KeycloakClaims,
    /// The request message (or request stream).
    pub message: T,
    token: Redacted<ForwardedToken>,
    info: CallInfo,
}

impl<T> RequestContext<T> {
    /// Build a context from a request. Only macro-generated code calls this.
    ///
    /// # Errors
    ///
    /// Fails closed with `Internal "authentication is not configured"` when
    /// the gate did not admit the request as authenticated (no [`RpcMethod`]
    /// or no verified caller). The detail is only logged.
    #[doc(hidden)]
    pub fn try_from_request(request: Request<T>) -> Result<Self, Status> {
        let (mut info, message) = CallInfo::from_request(request)?;

        // Taken out, not cloned: the token must not stay in the extensions
        // that the handler can see and print. A `Caller` can only come from
        // the gate (it is crate-private), so finding one is the proof of
        // authentication.
        let Some(Caller { claims, token }) = info.extensions.remove::<Caller>() else {
            return Err(not_configured(
                "request has no verified caller; the service is not served through \
                 `GrpcServer::add_service`, or its method is `#[public]` and takes a \
                 `RequestContext`",
            ));
        };

        Ok(Self {
            claims,
            message,
            token,
            info,
        })
    }

    /// A context for unit tests (`test-util` feature): method
    /// `test.Service/Test`, empty metadata, no peer and token `test-token`.
    /// Simulate more with [`with_rpc`](Self::with_rpc),
    /// [`with_remote_addr`](Self::with_remote_addr),
    /// [`with_metadata`](Self::with_metadata) and
    /// [`with_extension`](Self::with_extension). The
    /// [`testing`](crate::testing) module has an example.
    #[cfg(any(feature = "test-util", test))]
    pub fn for_test(claims: KeycloakClaims, message: T) -> Self {
        Self {
            claims,
            message,
            token: Redacted::from(ForwardedToken::new("test-token")),
            info: CallInfo::for_test(),
        }
    }

    /// Replace the RPC method of a [`for_test`](Self::for_test) context
    /// (`test-util` feature).
    #[cfg(any(feature = "test-util", test))]
    #[must_use = "this returns the updated context; the original is consumed"]
    pub fn with_rpc(mut self, rpc: RpcMethod) -> Self {
        self.info.rpc = rpc;
        self
    }

    /// Set the peer address of a [`for_test`](Self::for_test) context
    /// (`test-util` feature).
    #[cfg(any(feature = "test-util", test))]
    #[must_use = "this returns the updated context; the original is consumed"]
    pub fn with_remote_addr(mut self, remote_addr: std::net::SocketAddr) -> Self {
        self.info.remote_addr = Some(remote_addr);
        self
    }

    /// Replace the metadata of a [`for_test`](Self::for_test) context
    /// (`test-util` feature).
    #[cfg(any(feature = "test-util", test))]
    #[must_use = "this returns the updated context; the original is consumed"]
    pub fn with_metadata(mut self, metadata: tonic::metadata::MetadataMap) -> Self {
        self.info.metadata = metadata;
        self
    }

    /// Add a request extension to a [`for_test`](Self::for_test) context
    /// (`test-util` feature), replacing one of the same type.
    #[cfg(any(feature = "test-util", test))]
    #[must_use = "this returns the updated context; the original is consumed"]
    pub fn with_extension<E: Clone + Send + Sync + 'static>(mut self, extension: E) -> Self {
        self.info.extensions.insert(extension);
        self
    }

    /// The token subject (`sub` claim).
    pub fn sub(&self) -> &str {
        self.claims.sub()
    }

    /// The token expiry (`exp` claim) as seconds since the Unix epoch.
    pub fn exp_unix(&self) -> u64 {
        self.claims.exp_unix()
    }

    /// Whether the token has expired now. The gate checks expiry only when the
    /// call starts, so long-lived streams should re-check it.
    pub fn is_expired(&self) -> bool {
        self.claims.is_expired()
    }

    /// Whether the caller holds the given realm role (client roles never match).
    pub fn has_role(&self, role: &str) -> bool {
        self.claims.has_role(role)
    }

    /// The caller's token as a [`TokenProvider`], for a client that calls
    /// another service as the caller, such as a token-forwarding gateway:
    ///
    /// ```rust,no_run
    /// # use rust_grpc_lib::{GrpcClient, auth::RequestContext};
    /// # #[derive(GrpcClient)]
    /// # struct FooClient<T>(T);
    /// # impl<T> FooClient<T> { fn new(inner: T) -> Self { Self(inner) } }
    /// # fn f(ctx: &RequestContext<()>, url: &str) -> Result<(), rust_grpc_lib::pool::ChannelError> {
    /// let client = FooClient::authenticated(url, ctx.token())?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// The token is a secret; [`ForwardedToken`] has no `Debug` or `Display`.
    #[must_use = "this returns a copy of the token provider; it has no effect unless used"]
    pub fn token(&self) -> ForwardedToken {
        (*self.token).clone()
    }

    /// Wrap `message` in a `tonic::Request` that carries the caller's token in
    /// a sensitive `authorization` header, for a client without an
    /// interceptor. The header is omitted if the token is not a valid value.
    ///
    /// To forward the request message itself without a clone, use
    /// [`into_forwarded`](Self::into_forwarded).
    ///
    /// ```rust
    /// # #[cfg(feature = "test-util")] {
    /// # use rust_grpc_lib::{auth::RequestContext, testing::claims_with_roles};
    /// let ctx = RequestContext::for_test(claims_with_roles("alice", &[]), ());
    /// let request = ctx.forward("downstream message");
    /// assert_eq!(request.metadata().get("authorization").unwrap(), "Bearer test-token");
    /// # }
    /// ```
    #[must_use = "the request is built, not sent; pass it to a client"]
    pub fn forward<U>(&self, message: U) -> Request<U> {
        authorized(&self.token, message)
    }

    /// Consume the context and forward its own message: a `tonic::Request`
    /// holding [`message`](Self::message) and the caller's token in a
    /// sensitive `authorization` header (omitted if the token is not a valid
    /// value). Unlike `ctx.forward(ctx.message.clone())`, it does not clone
    /// the message. The claims and call details are dropped; read them
    /// first.
    ///
    /// ```rust
    /// # #[cfg(feature = "test-util")] {
    /// # use rust_grpc_lib::{auth::RequestContext, testing::claims_with_roles};
    /// let ctx = RequestContext::for_test(claims_with_roles("alice", &[]), vec![1, 2]);
    /// let request = ctx.into_forwarded();
    /// assert_eq!(request.get_ref(), &[1, 2]);
    /// assert_eq!(request.metadata().get("authorization").unwrap(), "Bearer test-token");
    /// # }
    /// ```
    #[must_use = "the request is built, not sent; pass it to a client"]
    pub fn into_forwarded(self) -> Request<T> {
        authorized(&self.token, self.message)
    }

    /// Consume the context into the message and the caller's token, for a
    /// client built with `authenticated` that needs both by
    /// value:
    ///
    /// ```rust
    /// # #[cfg(feature = "test-util")] {
    /// # use rust_grpc_lib::auth::{Claims, RequestContext, TokenProvider};
    /// # use rust_grpc_lib::testing::claims_with_roles;
    /// let ctx = RequestContext::for_test(claims_with_roles("alice", &[]), vec![1, 2]);
    /// let (message, token) = ctx.into_parts();
    /// assert_eq!(message, [1, 2]);
    /// assert_eq!(token.get_token().unwrap(), "test-token");
    /// # }
    /// ```
    ///
    /// The token is a secret; [`ForwardedToken`] has no `Debug` or `Display`.
    #[must_use = "this takes the context apart; dropping the parts discards them"]
    pub fn into_parts(self) -> (T, ForwardedToken) {
        (self.message, self.token.into_inner())
    }
}

// A request for `message` with the token in a sensitive `authorization`
// header; omitted if the token is not a valid header value.
fn authorized<U>(token: &Redacted<ForwardedToken>, message: U) -> Request<U> {
    let mut request = Request::new(message);
    if let Ok(token) = token.get_token()
        && let Ok(value) = bearer_header(&token)
    {
        request.metadata_mut().insert("authorization", value);
    }
    request
}

impl<T> Deref for RequestContext<T> {
    type Target = CallInfo;

    fn deref(&self) -> &CallInfo {
        &self.info
    }
}
