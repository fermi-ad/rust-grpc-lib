//! Server-side JWT validation layer for tonic gRPC services.
//!
//! The primary entry point is [`new_jwt_validation_layer`], which wraps any
//! [`TokenValidator`] in a [`tower::Layer`] that validates `Authorization: Bearer`
//! headers on every incoming request before the handler is called.
//!
//! On success, the validated [`KeycloakClaims`] are inserted into the request's
//! extensions so that handler methods can retrieve them via
//! `request.extensions().get::<KeycloakClaims>()`.

use std::sync::Arc;

use rust_auth_lib::TokenValidator;
use rust_auth_lib::keycloak::KeycloakClaims;
use tonic::{
    Request, Status,
    service::{Interceptor, InterceptorLayer},
};

#[cfg(test)]
mod tests;

/// A [`tower::Layer`] that installs JWT validation on a tonic server.
///
/// Wrap your tonic server with this layer to enforce that every incoming gRPC
/// request carries a valid `Authorization: Bearer <token>` header. Requests
/// that fail validation are rejected with [`tonic::Code::Unauthenticated`]
/// before reaching any handler.
///
/// `JwtValidationLayer` is a type alias for
/// `tonic::service::InterceptorLayer<JwtValidationService<V>>`. Because it is
/// a type alias, use the free function [`new_jwt_validation_layer`] to
/// construct it.
///
/// # Example
///
/// ```rust,ignore
/// use std::sync::Arc;
/// use rust_grpc_lib::auth::{
///     new_jwt_validation_layer, StaticKeysValidator, StaticKeysValidatorConfig,
/// };
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     // Reads AUTH_JWKS_FILE or AUTH_PEM_FILE; optionally AUTH_ISSUER
///     let validator = Arc::new(StaticKeysValidator::new(StaticKeysValidatorConfig::from_env()?)?);
///
///     tonic::transport::Server::builder()
///         .layer(new_jwt_validation_layer(validator))
///         .add_service(MyServiceServer::new(MyService))
///         .serve("[::1]:50051".parse()?)
///         .await?;
///
///     Ok(())
/// }
/// ```
pub type JwtValidationLayer<V> = InterceptorLayer<JwtValidationService<V>>;

/// Construct a [`JwtValidationLayer`] from the given validator.
///
/// This is a free function rather than an inherent `impl` because
/// [`JwtValidationLayer`] is a type alias for a tonic type and cannot have
/// methods added to it directly.
///
/// See [`JwtValidationLayer`] for a full usage example.
pub fn new_jwt_validation_layer<V>(validator: Arc<V>) -> JwtValidationLayer<V>
where
    V: TokenValidator + Send + Sync + 'static,
{
    InterceptorLayer::new(JwtValidationService::new(validator))
}

/// The server-side interceptor that validates `Authorization: Bearer <token>` on
/// every incoming gRPC request.
///
/// `JwtValidationService` is the inner interceptor type used by
/// [`JwtValidationLayer`]. You do not typically construct it directly; use
/// [`new_jwt_validation_layer`] or [`JwtValidationLayer::new`] instead.
///
/// On success, the validated [`KeycloakClaims`] are inserted into the request's
/// extensions so that handler methods can retrieve them:
///
/// ```rust,ignore
/// fn my_handler(&self, req: Request<MyRequest>) -> Result<Response<MyResponse>, Status> {
///     let claims = req.extensions().get::<KeycloakClaims>()
///         .ok_or_else(|| Status::unauthenticated("missing claims"))?;
///     // claims.has_role("admin"), claims.subject(), etc.
/// }
/// ```
///
/// On failure (missing header, expired token, wrong issuer, bad signature),
/// returns [`tonic::Status::unauthenticated`] immediately, which tonic converts
/// into a proper gRPC error response before the handler is ever called.
///
/// Because tonic's [`Interceptor`](tonic::service::Interceptor) trait is synchronous,
/// the async `validate` call is driven to completion with
/// [`tokio::task::block_in_place`]. This is safe when the validator's hot path is
/// cheap (e.g. a cached key lookup) and the caller is already inside a
/// multi-threaded Tokio runtime.
#[derive(Clone)]
pub struct JwtValidationService<V> {
    validator: Arc<V>,
}

impl<V: TokenValidator> JwtValidationService<V> {
    /// Create a new interceptor wrapping the given validator.
    pub fn new(validator: Arc<V>) -> Self {
        Self { validator }
    }
}

impl<V: TokenValidator + Send + Sync + 'static> Interceptor for JwtValidationService<V> {
    fn call(&mut self, mut req: Request<()>) -> Result<Request<()>, Status> {
        // Extract the Bearer token from the Authorization metadata entry.
        let token = req
            .metadata()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "))
            .map(|s| s.to_owned());

        let token = match token {
            Some(t) => t,
            None => {
                return Err(Status::unauthenticated("missing Authorization header"));
            }
        };

        // The Interceptor trait is synchronous; drive the async validator with
        // block_in_place so we don't block the executor thread.
        let claims = self
            .validator
            .validate::<KeycloakClaims>(&token)
            .map_err(|e| Status::unauthenticated(e.to_string()))?;

        req.extensions_mut().insert(claims);
        Ok(req)
    }
}
