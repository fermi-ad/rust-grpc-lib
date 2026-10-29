//! Server-side JWT validation for tonic gRPC services.
//!
//! Defines two types and three constructors:
//!
//! - [`JwtValidationService`] — the [`tonic::service::Interceptor`] that
//!   validates `Authorization: Bearer` headers on every incoming request.
//! - [`JwtValidationLayer`] — a type alias for
//!   `InterceptorLayer<JwtValidationService<V>>`.
//! - [`validator_into_layer`] — constructs a [`JwtValidationLayer`] from any
//!   [`TokenValidator`]. Applied with `Server::layer`, it guards **every**
//!   service on the server, including health checks and reflection.
//! - [`validator_into_interceptor`] (or [`JwtValidationService::new`]) —
//!   constructs the bare interceptor, for use with a generated server's
//!   `with_interceptor(inner, interceptor)`.
//! - [`wrap_service`] — wraps a single generated server in the interceptor
//!   without needing a direct `tower` dependency.
//!
//! # Global layer or per-service auth
//!
//! `Server::layer` wraps the whole router, so a global JWT layer also rejects
//! unauthenticated health probes. This module deliberately has **no
//! allow-list or exemption mechanism** for the global layer. To keep a service
//! such as `tonic-health` open, authenticate each application service
//! individually and add the open service unwrapped:
//!
//! ```rust,ignore
//! let (reporter, health) = tonic_health::server::health_reporter();
//! Server::builder()
//!     .add_service(health) // no JWT required
//!     .add_service(wrap_service::<KeycloakClaims, _, _>(
//!         MyServiceServer::new(MyService),
//!         validator,
//!     ))
//! ```
//!
//! The wrapped service keeps its `NamedService` implementation, so
//! `reporter.set_serving::<MyServiceServer<MyService>>()` continues to work.
//!
//! On successful validation, [`KeycloakClaims`](crate::auth::KeycloakClaims) are inserted into the request's
//! extensions. Handlers should read them through
//! [`RequestContext`](crate::auth::RequestContext). On failure the request is
//! rejected with [`tonic::Code::Unauthenticated`] before reaching any handler.

use std::marker::PhantomData;
use std::sync::Arc;

use rust_auth_lib::{Claims, TokenValidator};
use tonic::{
    Request, Status,
    service::{Interceptor, InterceptorLayer, interceptor::InterceptedService},
};

#[cfg(test)]
mod tests;

/// An [`InterceptorLayer`] that installs JWT validation on a tonic server.
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
///     validator_into_layer, KeycloakClaims, KeyValidator, KeyValidatorConfig,
/// };
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     // Reads AUTH_JWKS_FILE or AUTH_PEM_FILE; optionally AUTH_ISSUER
///     let validator = Arc::new(KeyValidator::new(KeyValidatorConfig::from_env()?)?);
///
///     tonic::transport::Server::builder()
///         .layer(validator_into_layer::<KeycloakClaims, _>(validator))
///         .add_service(MyServiceServer::new(MyService))
///         .serve("[::1]:50051".parse()?)
///         .await?;
///
///     Ok(())
/// }
/// ```
pub type JwtValidationLayer<C, V> = InterceptorLayer<JwtValidationService<C, V>>;

/// Construct a [`JwtValidationLayer`] that guards every service on a server.
///
/// See the [module docs](self) for why per-service auth
/// ([`wrap_service`]) is the right choice when some services must stay open.
pub fn validator_into_layer<C: Claims, V: TokenValidator>(
    validator: Arc<V>,
) -> JwtValidationLayer<C, V> {
    InterceptorLayer::new(validator_into_interceptor(validator))
}

/// Construct the bare [`JwtValidationService`] interceptor.
///
/// Pass it to a generated server's `with_interceptor`:
///
/// ```rust,ignore
/// let interceptor = validator_into_interceptor::<KeycloakClaims, _>(validator);
/// let svc = MyServiceServer::with_interceptor(MyService, interceptor);
/// ```
///
/// Tonic's `with_interceptor` takes `impl Interceptor`, and
/// `JwtValidationService` implements it for any [`Claims`] type that is
/// `Clone`.
pub fn validator_into_interceptor<C, V>(validator: Arc<V>) -> JwtValidationService<C, V> {
    JwtValidationService::new(validator)
}

/// Wrap a single tonic service so every request to it must carry a valid
/// `Authorization: Bearer` token.
///
/// This is the per-service alternative to [`validator_into_layer`]. It needs
/// no direct `tower` dependency, and the returned
/// [`InterceptedService`] forwards the inner service's
/// `tonic::server::NamedService` implementation, so it can be passed to
/// `add_service` and used with a health reporter's `set_serving`.
///
/// `C` is the claims type to insert into the request extensions; it is
/// usually [`KeycloakClaims`](crate::auth::KeycloakClaims):
///
/// ```rust,ignore
/// let svc = wrap_service::<KeycloakClaims, _, _>(MyServiceServer::new(MyService), validator);
/// ```
pub fn wrap_service<C, V, S>(
    service: S,
    validator: Arc<V>,
) -> InterceptedService<S, JwtValidationService<C, V>> {
    InterceptedService::new(service, validator_into_interceptor(validator))
}

/// The server-side interceptor that validates `Authorization: Bearer <token>` on
/// every incoming gRPC request.
///
/// `JwtValidationService` is the inner interceptor type used by
/// [`JwtValidationLayer`]. Construct it with [`JwtValidationService::new`] or
/// [`validator_into_interceptor`], or let [`validator_into_layer`] and
/// [`wrap_service`] do it for you.
///
/// On success, the validated [`KeycloakClaims`](crate::auth::KeycloakClaims) are inserted into the request's
/// extensions so that handler methods can retrieve them through
/// [`RequestContext`](crate::auth::RequestContext):
///
/// ```rust,ignore
/// async fn my_handler(&self, req: Request<MyRequest>) -> Result<Response<MyResponse>, Status> {
///     let ctx = RequestContext::try_from_request(req)?;
///     // ctx.has_role("admin"), ctx.sub(), ctx.message, etc.
/// }
/// ```
///
/// On failure (missing header, expired token, wrong issuer, bad signature),
/// returns [`tonic::Status::unauthenticated`] immediately, which tonic converts
/// into a proper gRPC error response before the handler is ever called.
pub struct JwtValidationService<C, V> {
    _claims: PhantomData<fn() -> C>,
    validator: Arc<V>,
}

impl<C, V> JwtValidationService<C, V> {
    /// Create an interceptor that validates tokens with `validator` and
    /// inserts claims of type `C` into the request extensions.
    pub fn new(validator: Arc<V>) -> Self {
        Self {
            _claims: PhantomData,
            validator,
        }
    }
}

// Manual impl: `#[derive(Clone)]` would needlessly require `C: Clone` and
// `V: Clone`, but only the `Arc<V>` is cloned.
impl<C, V> Clone for JwtValidationService<C, V> {
    fn clone(&self) -> Self {
        Self {
            _claims: PhantomData,
            validator: Arc::clone(&self.validator),
        }
    }
}

impl<C: Claims + Clone, V: TokenValidator + Send + Sync + 'static> Interceptor
    for JwtValidationService<C, V>
{
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

        // The Interceptor trait is synchronous, and `TokenValidator::validate`
        // is a synchronous call too, so the claims are available immediately.
        let claims = self
            .validator
            .validate::<C>(&token)
            .map_err(|e| Status::unauthenticated(e.to_string()))?;

        req.extensions_mut().insert(claims);
        Ok(req)
    }
}
