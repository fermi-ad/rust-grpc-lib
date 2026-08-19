//! gRPC client interceptors for attaching JWT `Authorization` headers to
//! outbound requests.
//!
//! Two interceptors are provided:
//!
//! - [`ForwardedTokenInterceptor`] — captures the `Authorization` header from
//!   an incoming tonic request and re-attaches it to every outbound request.
//!   Used by middle-layer services to propagate the caller's identity downstream.
//! - [`ClientJwtInterceptor`] — calls any [`TokenProvider`] on each outbound
//!   request and attaches the result as a `Bearer` token. Used internally by
//!   [`crate::pool::get`].

use rust_auth_lib::{TokenError, TokenProvider};
use tonic::{
    Request, Status,
    metadata::{Ascii, MetadataValue},
    service::Interceptor,
};

#[cfg(test)]
mod tests;

/// Forwards the `Authorization: Bearer` token from an incoming gRPC request to
/// all outbound gRPC calls made during the same handler invocation.
///
/// # Use case — middle-layer services
///
/// A middle-layer service receives a validated user JWT from the gateway and
/// must propagate it to downstream gRPC services so that the full call chain
/// operates under the same user identity. `ForwardedTokenInterceptor` captures
/// the `Authorization` header from the incoming [`tonic::Request`] and re-attaches
/// it to every outbound request made via [`crate::pool::get`].
///
/// ```rust,ignore
/// #[rust_grpc_lib::grpc_service]
/// impl Daq for MyDaqService {
///     async fn get_data(&self, req: Request<GetDataRequest>) -> Result<Response<GetDataResponse>, Status> {
///         let client: AlarmCommandsClient<_> = rust_grpc_lib::pool::get(
///             "http://alarm-host:50051",
///             ForwardedTokenInterceptor::from_request(&req),
///         )?;
///         // ...
///     }
/// }
/// ```
///
/// Also implements [`TokenProvider`] so it can be passed directly to
/// [`crate::pool::get`].
pub struct ForwardedTokenInterceptor {
    token: Option<MetadataValue<Ascii>>,
}

impl ForwardedTokenInterceptor {
    /// Construct from the metadata of an incoming tonic request.
    pub fn from_request<T>(request: &Request<T>) -> Self {
        Self {
            token: request.metadata().get("authorization").cloned(),
        }
    }
}

impl Interceptor for ForwardedTokenInterceptor {
    fn call(&mut self, mut req: Request<()>) -> Result<Request<()>, Status> {
        if let Some(token) = &self.token {
            req.metadata_mut().insert("authorization", token.clone());
        }
        Ok(req)
    }
}

impl TokenProvider for ForwardedTokenInterceptor {
    fn get_token(&self) -> Result<String, TokenError> {
        self.token
            .as_ref()
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "))
            .map(|s| s.to_owned())
            .ok_or(TokenError::EmptyAccessToken)
    }
}

/// Generic interceptor that calls any [`TokenProvider`] and attaches the result
/// as a `Bearer` token on every outbound gRPC request.
///
/// Used internally by [`crate::pool::get`]; consumers do not typically construct
/// this directly.
pub struct ClientJwtInterceptor<P: TokenProvider> {
    pub(crate) provider: P,
}

impl<P: TokenProvider> Interceptor for ClientJwtInterceptor<P> {
    fn call(&mut self, mut req: Request<()>) -> Result<Request<()>, Status> {
        let token = self
            .provider
            .get_token()
            .map_err(|e| Status::unauthenticated(e.to_string()))?;

        req.metadata_mut()
            .insert("authorization", format!("Bearer {token}").parse().unwrap());
        Ok(req)
    }
}
