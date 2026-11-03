//! A server builder that wires authentication, health, tracing and shutdown
//! the way this library recommends.
//!
//! [`GrpcServer::builder`] returns a [`GrpcServerBuilder`] that does the
//! plumbing every service otherwise repeats by hand:
//!
//! - **Per-service authentication.** Every service added with
//!   [`add_service`](GrpcServerBuilder::add_service) is wrapped with
//!   [`wrap_service`](crate::auth::wrap_service), so a missing or invalid token
//!   is rejected with `Unauthenticated` before any handler runs.
//! - **An open health service.** `grpc.health.v1.Health` is added *unwrapped*, so
//!   probes work without a token. The overall service (`""`) and each added
//!   service are marked `Serving` when the server starts and `NotServing` as soon
//!   as the shutdown signal fires, before connections start draining.
//! - **A response timeout**, set with [`timeout`](GrpcServerBuilder::timeout).
//! - **Tracing.** With the `trace` feature the builder installs
//!   `GrpcTraceLayer` as the outermost layer, so every RPC (health and
//!   authentication rejections included) gets an `rpc` span.
//!
//! Enabled by the `server` feature (off by default).
//!
//! # Choosing an authentication mode
//!
//! The builder starts in the [`AuthUnset`] state, where services cannot be
//! added and the server cannot be started. You must pick a mode first:
//!
//! | Method | Resulting state | Notes |
//! |---|---|---|
//! | [`authenticate`](GrpcServerBuilder::authenticate) | [`Authenticated`] | Use an existing validator; claims are `KeycloakClaims` |
//! | [`authenticate_with_claims`](GrpcServerBuilder::authenticate_with_claims) | [`Authenticated`] | Same, with a custom claims type |
//! | [`authenticate_with_config`](GrpcServerBuilder::authenticate_with_config) | [`Authenticated`] | Builds the `KeyValidator` from a `KeyValidatorConfig` |
//! | `dangerously_unauthenticated` | [`Unauthenticated`] | Requires the `unauthenticated` feature; for test harnesses |
//!
//! Forgetting this step is a compile error, not a server that silently accepts
//! every caller.
//!
//! # Example
//!
//! ```rust,ignore
//! use rust_grpc_lib::auth::KeyValidatorConfig;
//! use rust_grpc_lib::server::GrpcServer;
//!
//! let config = KeyValidatorConfig::from_jwks_url(jwks_url, Duration::from_secs(300));
//! GrpcServer::builder()
//!     .timeout(Duration::from_secs(30))
//!     .authenticate_with_config(config)?
//!     .add_service(MyServiceServer::new(MyService))
//!     .serve_on_listener(listener, token.cancelled())
//!     .await?;
//! ```
//!
//! # What this module does not do
//!
//! Every application service is authenticated; there is deliberately no way to
//! add an open service next to authenticated ones (an escape hatch would be an
//! easy way to leave an RPC unprotected by accident). Health is the one open
//! service. Per-method policy stays with
//! [`keycloak_authenticated_service`](crate::keycloak_authenticated_service).
//!
//! Role checks and the claims a handler sees are unchanged: handlers read them
//! through [`RequestContext`](crate::auth::RequestContext).

use std::convert::Infallible;
use std::error::Error;
use std::fmt;
use std::future::{Future, pending};
use std::io;
use std::marker::PhantomData;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;
use tonic::body::Body;
use tonic::codegen::Service;
use tonic::codegen::http::{Request, Response};
use tonic::server::NamedService;
use tonic::service::Routes;
use tonic::transport::Server;
use tonic::transport::server::TcpIncoming;
use tonic_health::ServingStatus;
use tonic_health::server::{HealthReporter, health_reporter};

use crate::auth::{
    AuthError, Claims, KeyValidator, KeyValidatorConfig, KeycloakClaims, TokenValidator,
    wrap_service,
};
#[cfg(feature = "trace")]
use crate::trace::GrpcTraceLayer;

#[cfg(test)]
mod tests;

mod sealed {
    pub trait Sealed {}
}

/// Entry point of the server builder. See the [module docs](self).
#[derive(Debug, Clone, Copy)]
pub struct GrpcServer;

impl GrpcServer {
    /// Start building a server. Choose an authentication mode next; see the
    /// [module docs](self).
    pub fn builder() -> GrpcServerBuilder<AuthUnset> {
        let (reporter, health_service) = health_reporter();
        GrpcServerBuilder {
            auth: AuthUnset,
            timeout: None,
            routes: Routes::new(health_service),
            health: reporter,
            served: Vec::new(),
        }
    }
}

/// A tonic service that the builder can serve: what a generated `FooServer<T>`
/// is.
///
/// This is a blanket alias for the bounds tonic puts on `Router::add_service`,
/// spelled with concrete request and response types so that it can be written
/// without naming `axum`. Every generated server implements it; you do not
/// implement it yourself.
pub trait GrpcService:
    Service<Request<Body>, Response = Response<Body>, Error = Infallible, Future: Send + 'static>
    + NamedService
    + Clone
    + Send
    + Sync
    + 'static
{
}

impl<S> GrpcService for S where
    S: Service<
            Request<Body>,
            Response = Response<Body>,
            Error = Infallible,
            Future: Send + 'static,
        > + NamedService
        + Clone
        + Send
        + Sync
        + 'static
{
}

/// How a builder protects the services added to it: [`Authenticated`] or
/// [`Unauthenticated`]. Sealed; [`AuthUnset`] deliberately does not implement
/// it, which is why a builder that has not chosen cannot add services or
/// serve (rustc reports `AuthUnset: AuthMode` as an unsatisfied bound on
/// `add_service`).
pub trait AuthMode: sealed::Sealed {
    /// Add `service` to `routes`, protected as this mode requires.
    #[doc(hidden)]
    fn register<S: GrpcService>(&self, routes: Routes, service: S) -> Routes;
}

/// Builder state: no authentication mode chosen yet.
#[derive(Debug, Clone, Copy)]
pub struct AuthUnset;

/// Builder state: every added service requires a valid token.
pub struct Authenticated<C, V> {
    validator: Arc<V>,
    _claims: PhantomData<fn() -> C>,
}

/// Builder state: services are added without any authentication.
///
/// Only reachable through `dangerously_unauthenticated`, behind the
/// `unauthenticated` feature.
#[cfg(feature = "unauthenticated")]
#[derive(Debug, Clone, Copy)]
pub struct Unauthenticated;

/// The error returned when the server fails to start or stops abnormally.
#[derive(Debug)]
#[non_exhaustive]
pub enum ServeError {
    /// The listen address could not be bound, for example because the port is
    /// already in use.
    Bind(io::Error),
    /// tonic's transport failed while serving.
    Transport(tonic::transport::Error),
}

/// Builds a gRPC server; see the [module docs](self).
///
/// `A` is the authentication state. [`add_service`](Self::add_service) and the
/// `serve*` methods exist only once it is [`Authenticated`] or
/// [`Unauthenticated`].
pub struct GrpcServerBuilder<A> {
    auth: A,
    timeout: Option<Duration>,
    routes: Routes,
    health: HealthReporter,
    served: Vec<&'static str>,
}

impl<A> GrpcServerBuilder<A> {
    /// Fail any RPC that takes longer than `timeout` to produce a response.
    ///
    /// This is tonic's `Server::timeout`: the handler future is dropped at the
    /// deadline and the client receives `Cancelled`. A shorter `grpc-timeout`
    /// header from the client still takes precedence.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }
}

impl GrpcServerBuilder<AuthUnset> {
    /// Authenticate every service with `validator`, inserting
    /// [`KeycloakClaims`] into the request extensions.
    pub fn authenticate<V>(
        self,
        validator: Arc<V>,
    ) -> GrpcServerBuilder<Authenticated<KeycloakClaims, V>>
    where
        V: TokenValidator + Send + Sync + 'static,
    {
        self.authenticate_with_claims(validator)
    }

    /// Like [`authenticate`](Self::authenticate), with a custom claims type
    /// `C`. Use the same type in `#[keycloak_authenticated_service(claims = C)]`.
    pub fn authenticate_with_claims<C, V>(
        self,
        validator: Arc<V>,
    ) -> GrpcServerBuilder<Authenticated<C, V>>
    where
        C: Claims + Clone + 'static,
        V: TokenValidator + Send + Sync + 'static,
    {
        self.with_auth(Authenticated {
            validator,
            _claims: PhantomData,
        })
    }

    /// Build a [`KeyValidator`] from `config` and authenticate every service
    /// with it. Fails if the config is invalid (an unreadable key file, for
    /// example), so a misconfigured server never starts.
    pub fn authenticate_with_config(
        self,
        config: KeyValidatorConfig,
    ) -> Result<GrpcServerBuilder<Authenticated<KeycloakClaims, KeyValidator>>, AuthError> {
        let validator = Arc::new(KeyValidator::new(config)?);
        Ok(self.authenticate(validator))
    }

    /// Serve every service with **no authentication at all**.
    ///
    /// For test harnesses only. Requires the `unauthenticated` feature, which
    /// should not be enabled in a production build.
    #[cfg(feature = "unauthenticated")]
    pub fn dangerously_unauthenticated(self) -> GrpcServerBuilder<Unauthenticated> {
        self.with_auth(Unauthenticated)
    }

    fn with_auth<B>(self, auth: B) -> GrpcServerBuilder<B> {
        GrpcServerBuilder {
            auth,
            timeout: self.timeout,
            routes: self.routes,
            health: self.health,
            served: self.served,
        }
    }
}

impl<A: AuthMode> GrpcServerBuilder<A> {
    /// Add a generated server (`FooServer::new(my_impl)`), protected as the
    /// chosen authentication mode requires, and report it in the health
    /// service.
    pub fn add_service<S: GrpcService>(mut self, service: S) -> Self {
        self.routes = self.auth.register(self.routes, service);
        self.served.push(S::NAME);
        self
    }

    /// Serve on `addr` until the server fails. See
    /// [`serve_with_shutdown`](Self::serve_with_shutdown) for graceful
    /// shutdown.
    pub async fn serve(self, addr: SocketAddr) -> Result<(), ServeError> {
        self.serve_with_shutdown(addr, pending()).await
    }

    /// Serve on `addr` until `shutdown` completes, then drain gracefully.
    ///
    /// Health is switched to `NotServing` first, so load balancers stop
    /// sending traffic while in-flight RPCs finish. Pass a cancellation
    /// token's `cancelled()` future, `ctrl_c()`, or similar.
    pub async fn serve_with_shutdown(
        self,
        addr: SocketAddr,
        shutdown: impl Future<Output = ()>,
    ) -> Result<(), ServeError> {
        let incoming = TcpIncoming::bind(addr).map_err(ServeError::Bind)?;
        self.run(incoming, shutdown).await
    }

    /// Like [`serve_with_shutdown`](Self::serve_with_shutdown), on a listener
    /// you already bound.
    ///
    /// Bind to port `0` and read `local_addr()` first to get an OS-assigned
    /// port, which is what tests want.
    pub async fn serve_on_listener(
        self,
        listener: TcpListener,
        shutdown: impl Future<Output = ()>,
    ) -> Result<(), ServeError> {
        self.run(TcpIncoming::from(listener), shutdown).await
    }

    async fn run(
        self,
        incoming: TcpIncoming,
        shutdown: impl Future<Output = ()>,
    ) -> Result<(), ServeError> {
        let Self {
            timeout,
            routes,
            health,
            served,
            ..
        } = self;

        set_status(&health, &served, ServingStatus::Serving).await;
        let signal = async move {
            shutdown.await;
            set_status(&health, &served, ServingStatus::NotServing).await;
        };

        let mut server = Server::builder();
        if let Some(timeout) = timeout {
            server = server.timeout(timeout);
        }
        // Layers wrap every service, so the trace layer goes first to be
        // outermost, and layers must be applied before `add_routes`.
        #[cfg(feature = "trace")]
        let router = server.layer(GrpcTraceLayer::new()).add_routes(routes);
        #[cfg(not(feature = "trace"))]
        let router = server.add_routes(routes);

        router
            .serve_with_incoming_shutdown(incoming.with_nodelay(Some(true)), signal)
            .await?;
        Ok(())
    }
}

impl<C, V> sealed::Sealed for Authenticated<C, V> {}

impl<C, V> AuthMode for Authenticated<C, V>
where
    C: Claims + Clone + 'static,
    V: TokenValidator + Send + Sync + 'static,
{
    fn register<S: GrpcService>(&self, routes: Routes, service: S) -> Routes {
        routes.add_service(wrap_service::<C, V, S>(
            service,
            Arc::clone(&self.validator),
        ))
    }
}

#[cfg(feature = "unauthenticated")]
impl sealed::Sealed for Unauthenticated {}

#[cfg(feature = "unauthenticated")]
impl AuthMode for Unauthenticated {
    fn register<S: GrpcService>(&self, routes: Routes, service: S) -> Routes {
        routes.add_service(service)
    }
}

impl fmt::Display for ServeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bind(source) => write!(f, "failed to bind the listen address: {source}"),
            Self::Transport(source) => write!(f, "gRPC transport error: {source}"),
        }
    }
}

impl Error for ServeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Bind(source) => Some(source),
            Self::Transport(source) => Some(source),
        }
    }
}

impl From<tonic::transport::Error> for ServeError {
    fn from(source: tonic::transport::Error) -> Self {
        Self::Transport(source)
    }
}

/// Set the overall service (`""`) and every added service to `status`.
async fn set_status(health: &HealthReporter, services: &[&'static str], status: ServingStatus) {
    health.set_service_status("", status).await;
    for service in services {
        health.set_service_status(service, status).await;
    }
}
