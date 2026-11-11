//! [`ClientJwtInterceptor`]: attaches a JWT to every outgoing request.

use rust_auth_lib::TokenProvider;
use tonic::{
    Request, Status,
    metadata::{Ascii, MetadataValue, errors::InvalidMetadataValue},
    service::{Interceptor, interceptor::InterceptedService},
    transport::Channel,
};

#[cfg(test)]
mod tests;

/// The transport of a client built with `authenticated`: a
/// pooled [`Channel`] behind a [`ClientJwtInterceptor`]. Name it to store
/// such a client, e.g. `ExampleDbClient<AuthenticatedClient<ForwardedToken>>`.
pub type AuthenticatedClient<P> = InterceptedService<Channel, ClientJwtInterceptor<P>>;

/// A tonic [`Interceptor`] that sends a [`TokenProvider`]'s token as a
/// sensitive `authorization: Bearer <token>` header on every call.
///
/// Generated clients install it through `authenticated`. To
/// forward a caller's token, pass `RequestContext::token` as the provider.
///
/// # Errors
///
/// Fails the call with `Unauthenticated` if the provider errors, and with
/// `Internal` if the token is not a valid header value.
pub struct ClientJwtInterceptor<P: TokenProvider> {
    provider: P,
}

impl<P: TokenProvider> ClientJwtInterceptor<P> {
    /// Create an interceptor that asks `provider` for a token on every call.
    ///
    /// ```rust
    /// use rust_grpc_lib::auth::{ClientJwtInterceptor, ForwardedToken};
    /// use rust_grpc_lib::tonic::service::Interceptor;
    /// let mut interceptor = ClientJwtInterceptor::new(ForwardedToken::new("my-token"));
    /// let request = interceptor.call(rust_grpc_lib::tonic::Request::new(())).unwrap();
    /// assert_eq!(request.metadata().get("authorization").unwrap(), "Bearer my-token");
    /// ```
    pub fn new(provider: P) -> Self {
        Self { provider }
    }
}

impl<P: TokenProvider> Interceptor for ClientJwtInterceptor<P> {
    fn call(&mut self, mut req: Request<()>) -> Result<Request<()>, Status> {
        let token = self
            .provider
            .get_token()
            .map_err(|e| Status::unauthenticated(e.to_string()))?;
        let value = bearer_header(&token).map_err(|e| {
            Status::internal(format!("token contains invalid header characters: {e}"))
        })?;

        req.metadata_mut().insert("authorization", value);
        Ok(req)
    }
}

// The `authorization` value for `token`, marked sensitive so `Debug` hides
// it. Shared by the interceptor and `RequestContext::forward`.
pub(crate) fn bearer_header(token: &str) -> Result<MetadataValue<Ascii>, InvalidMetadataValue> {
    let mut value: MetadataValue<Ascii> = format!("Bearer {token}").parse()?;
    value.set_sensitive(true);
    Ok(value)
}
