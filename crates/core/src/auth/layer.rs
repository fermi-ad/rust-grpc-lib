//! Server-side JWT validation layer for tonic gRPC services.
//!
//! Defines two types and one constructor:
//!
//! - [`JwtValidationService`] — the [`tonic::service::Interceptor`] that
//!   validates `Authorization: Bearer` headers on every incoming request.
//! - [`JwtValidationLayer`] — a type alias for
//!   `InterceptorLayer<JwtValidationService<V>>`.
//! - [`validator_into_layer`] — constructs a [`JwtValidationLayer`] from any
//!   [`TokenValidator`].
//!
//! On successful validation, [`KeycloakClaims`] are inserted into the request's
//! extensions so handler methods can retrieve them via
//! `request.extensions().get::<KeycloakClaims>()`. On failure the request is
//! rejected with [`tonic::Code::Unauthenticated`] before reaching any handler.

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
/// a type alias, use the free function [`validator_into_layer`] to construct it.
///
/// # Example
///
/// ```rust,ignore
/// use std::sync::Arc;
/// use rust_grpc_lib::auth::{
///     validator_into_layer, StaticKeysValidator, StaticKeysValidatorConfig,
/// };
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     // Reads AUTH_JWKS_FILE or AUTH_PEM_FILE; optionally AUTH_ISSUER
///     let validator = Arc::new(StaticKeysValidator::new(StaticKeysValidatorConfig::from_env()?)?);
///
///     tonic::transport::Server::builder()
///         .layer(validator_into_layer(validator))
///         .add_service(MyServiceServer::new(MyService))
///         .serve("[::1]:50051".parse()?)
///         .await?;
///
///     Ok(())
/// }
/// ```
pub type JwtValidationLayer<V> = InterceptorLayer<JwtValidationService<V>>;

pub fn validator_into_layer<V: TokenValidator>(validator: Arc<V>) -> JwtValidationLayer<V> {
    InterceptorLayer::new(JwtValidationService { validator })
}

/// The server-side interceptor that validates `Authorization: Bearer <token>` on
/// every incoming gRPC request.
///
/// `JwtValidationService` is the inner interceptor type used by
/// [`JwtValidationLayer`]. You do not typically construct it directly; use
/// [`validator_into_layer`] instead.
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
#[derive(Clone)]
pub struct JwtValidationService<V> {
    validator: Arc<V>,
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
