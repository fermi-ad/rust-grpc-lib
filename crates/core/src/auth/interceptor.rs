//! [`ClientJwtInterceptor`] — outbound tonic interceptor that attaches a JWT
//! `Authorization: Bearer` header to every request by calling a [`TokenProvider`].
//!
//! This is the only type defined in this module. It is used internally by the
//! `from_endpoint_with_provider` constructor that `#[derive(GrpcClient)]`
//! generates on every client struct; consumers do not construct it directly.

use rust_auth_lib::TokenProvider;
use tonic::{Request, Status, service::Interceptor};

/// Generic interceptor that calls any [`TokenProvider`] and attaches the result
/// as a `Bearer` token on every outbound gRPC request.
///
/// Used internally by the `from_endpoint_with_provider` constructor that
/// `#[derive(GrpcClient)]` generates on every client struct; consumers do not
/// typically construct this directly.
///
/// To forward the caller's token from an incoming request to a downstream
/// service, use [`crate::auth::extract_token`] to obtain a [`rust_auth_lib::ForwardedToken`]
/// and pass it as the provider:
///
/// ```rust,ignore
/// #[rust_grpc_lib::grpc_service]
/// impl Daq for MyDaqService {
///     async fn get_data(&self, req: Request<GetDataRequest>) -> Result<Response<GetDataResponse>, Status> {
///         let provider = rust_grpc_lib::auth::extract_token(&req)?;
///         let client = AlarmCommandsClient::from_endpoint_with_provider(
///             "http://alarm-host:50051",
///             provider,
///         )?;
///         // ...
///     }
/// }
/// ```
pub struct ClientJwtInterceptor<P: TokenProvider> {
    provider: P,
}

impl<P: TokenProvider> ClientJwtInterceptor<P> {
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

        req.metadata_mut().insert(
            "authorization",
            format!("Bearer {token}").parse().map_err(|e| {
                Status::internal(format!("token contains invalid header characters: {e}"))
            })?,
        );
        Ok(req)
    }
}
