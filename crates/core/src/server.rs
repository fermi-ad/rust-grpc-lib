//! Serve gRPC services behind the access gate, with health checks, per-RPC
//! tracing, an optional timeout and graceful shutdown (`server` feature).
//!
//! [`GrpcServer::builder`] is the only way to serve services with this crate.
//! Each RPC passes through the trace layer (outermost), then the service's
//! gate, then the generated server.
//!
//! - **Access control.** [`add_service`](GrpcServerBuilder::add_service)
//!   only accepts implementations annotated with
//!   [`#[access_controlled_service]`](crate::access_controlled_service), and
//!   their per-method policies are enforced before any handler runs. Auth
//!   cannot be switched off: the only open endpoints are `#[public]` methods
//!   and health.
//! - **Health.** `grpc.health.v1.Health` needs no token. The overall service
//!   (`""`) and every added service report `Serving` once the server starts
//!   and `NotServing` as soon as the shutdown signal fires, before draining.
//! - **Tracing.** Every RPC, including health calls and rejections, gets an
//!   `rpc` span on target `rust_grpc_lib::trace` with the fields `rpc.system`,
//!   `rpc.service`, `rpc.method`, `grpc.status`, `latency_ms`, `stream_ms`,
//!   `access` and `user`. The gate records `access` (not for health,
//!   unparsable paths or unknown methods) and `user` (once a token
//!   validates). A response dropped before it is produced (client cancel or
//!   timeout) records `grpc.status` as `Cancelled`; a cancel mid-stream
//!   leaves it unset.
//!
//! # Example
//!
//! The validator is typically a `KeyValidator`; tests can use
//! `testing::test_validator` (`test-util` feature). For a runnable server
//! with a real generated service, see the
//! [service template](https://github.com/fermi-ad/rust-grpc-service-template);
//! `docs/server.md` covers policies, gate status codes and log filtering.
//!
//! ```rust,no_run
//! # use std::{future::Future, sync::Arc, time::Duration};
//! # use rust_grpc_lib::auth::{ServicePolicy, TokenValidator};
//! # use rust_grpc_lib::server::{GrpcServer, GrpcService, ServeError};
//! # async fn run<V: TokenValidator, T: ServicePolicy<S>, S: GrpcService>(
//! #     validator: Arc<V>,
//! #     new_server: fn(T) -> S,
//! #     my_impl: T,
//! #     shutdown: impl Future<Output = ()>,
//! # ) -> Result<(), ServeError> {
//! // `new_server` is a generated constructor such as `FooServer::new`.
//! GrpcServer::builder(validator)
//!     .timeout(Duration::from_secs(30))
//!     .add_service(new_server, my_impl)
//!     .serve_with_shutdown("0.0.0.0:50051".parse().unwrap(), shutdown)
//!     .await
//! # }
//! ```
//!
//! # Serve methods
//!
//! | Method | Returns | Notes |
//! |---|---|---|
//! | [`serve`](GrpcServerBuilder::serve) | `Result<Infallible, ServeError>` | Binds; never returns `Ok` |
//! | [`serve_with_shutdown`](GrpcServerBuilder::serve_with_shutdown) | `Result<(), ServeError>` | Binds; `Ok(())` after graceful shutdown |
//! | [`serve_on_listener`](GrpcServerBuilder::serve_on_listener) | `Result<(), ServeError>` | Your listener; never `ServeError::Bind` |
//!
//! All three return [`ServeError`], so `?` unifies them in one function.

use std::convert::Infallible;
use std::error::Error;
use std::fmt;
use std::future::{Future, pending};
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;
use tonic::body::Body;
use tonic::codegen::Service;
use tonic::codegen::http::{Request, Response};
use tonic::server::NamedService;
use tonic::service::Routes;
use tonic::transport::server::TcpIncoming;
use tonic::transport::{Error as TransportError, Server};
use tonic_health::ServingStatus;
use tonic_health::server::{HealthReporter, health_reporter};

use crate::auth::gate::AuthGate;
use crate::auth::{PolicyTable, ServicePolicy, TokenValidator};
use crate::trace::GrpcTraceLayer;

#[cfg(test)]
mod tests;

/// The bounds a tonic service needs to be served, such as a generated
/// `FooServer<T>`. Blanket-implemented; never implement it by hand.
// Mirrors the bounds of tonic's `Router::add_service`, spelled with concrete
// request and response types so the signature does not need to name `axum`.
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

/// Entry point for building a server. See the [module docs](self).
#[derive(Debug, Clone, Copy)]
pub struct GrpcServer;

impl GrpcServer {
    /// Starts a builder that authenticates calls with `validator`.
    #[must_use = "a builder does nothing until services are added and `serve*` is called"]
    pub fn builder<V: TokenValidator>(validator: Arc<V>) -> GrpcServerBuilder<V> {
        let (reporter, health_service) = health_reporter();
        GrpcServerBuilder {
            validator,
            timeout: None,
            routes: Routes::new(health_service),
            health: reporter,
            served: Vec::new(),
        }
    }
}

/// Configures and runs a server; created by [`GrpcServer::builder`].
pub struct GrpcServerBuilder<V> {
    validator: Arc<V>,
    timeout: Option<Duration>,
    routes: Routes,
    health: HealthReporter,
    served: Vec<&'static str>,
}

impl<V: TokenValidator> GrpcServerBuilder<V> {
    /// Fails any RPC that takes longer than `timeout` to produce a response.
    ///
    /// The handler is dropped at the deadline and the client gets
    /// `Cancelled`. A shorter client `grpc-timeout` still wins. Default: none.
    #[must_use = "this consumes the builder and returns an updated one; the timeout is lost if the result is dropped"]
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Adds a service behind the gate and reports it through health:
    /// `.add_service(FooServer::new, MyFoo)`.
    ///
    /// # Compile errors
    ///
    /// `T: ServicePolicy<S>` is unsatisfied if `T`'s `impl` block lacks
    /// [`#[access_controlled_service]`](crate::access_controlled_service), or
    /// if `constructor` belongs to a different service than that block.
    #[must_use = "this consumes the builder and returns an updated one; the service is lost if the result is dropped"]
    pub fn add_service<T, S>(mut self, constructor: impl FnOnce(T) -> S, implementation: T) -> Self
    where
        T: ServicePolicy<S>,
        S: GrpcService,
    {
        let gate = AuthGate::new(
            constructor(implementation),
            PolicyTable::of::<S, T>(),
            Arc::clone(&self.validator),
        );
        self.routes = self.routes.add_service(gate);
        self.served.push(S::NAME);
        self
    }

    /// Serves on `addr` until the server fails.
    ///
    /// The success type is [`Infallible`], so `serve(addr).await?` needs no
    /// further handling and `let Err(err) = serve(addr).await;` is
    /// exhaustive.
    ///
    /// # Errors
    ///
    /// [`ServeError::Bind`] if `addr` cannot be bound (for example, the port
    /// is in use); [`ServeError::Transport`] if the transport fails.
    pub async fn serve(self, addr: SocketAddr) -> Result<Infallible, ServeError> {
        match self.serve_with_shutdown(addr, pending()).await {
            // `pending()` never completes, so the server cannot return `Ok`.
            Ok(()) => unreachable!("the shutdown future passed to `serve` never completes"),
            Err(err) => Err(err),
        }
    }

    /// Serves on `addr` until `shutdown` completes (for example
    /// `tokio::signal::ctrl_c()` or a cancellation token's `cancelled()`),
    /// then drains gracefully.
    ///
    /// Health flips to `NotServing` before the drain, so load balancers stop
    /// sending traffic while in-flight RPCs finish. Open streams, including
    /// health `Watch`, delay completion.
    ///
    /// # Errors
    ///
    /// As for [`serve`](Self::serve). A completed shutdown returns `Ok(())`.
    pub async fn serve_with_shutdown(
        self,
        addr: SocketAddr,
        shutdown: impl Future<Output = ()>,
    ) -> Result<(), ServeError> {
        let incoming = TcpIncoming::bind(addr).map_err(ServeError::Bind)?;
        self.run(incoming, shutdown)
            .await
            .map_err(ServeError::Transport)
    }

    /// [`serve_with_shutdown`](Self::serve_with_shutdown) on a listener you
    /// already bound. In tests, bind port `0` and read `local_addr()` first.
    ///
    /// # Errors
    ///
    /// [`ServeError::Transport`] if the transport fails; there is no bind
    /// step, so never [`ServeError::Bind`]. A completed shutdown returns
    /// `Ok(())`.
    pub async fn serve_on_listener(
        self,
        listener: TcpListener,
        shutdown: impl Future<Output = ()>,
    ) -> Result<(), ServeError> {
        self.run(TcpIncoming::from(listener), shutdown)
            .await
            .map_err(ServeError::Transport)
    }

    async fn run(
        self,
        incoming: TcpIncoming,
        shutdown: impl Future<Output = ()>,
    ) -> Result<(), TransportError> {
        let Self {
            timeout,
            routes,
            health,
            served,
            ..
        } = self;

        set_status(&health, &served, ServingStatus::Serving).await;
        // Flip health before the graceful drain begins so probes see
        // `NotServing` while in-flight RPCs finish.
        let signal = async move {
            shutdown.await;
            set_status(&health, &served, ServingStatus::NotServing).await;
        };

        let mut server = Server::builder();
        if let Some(timeout) = timeout {
            server = server.timeout(timeout);
        }
        // Layers wrap every service, so the trace layer goes first to be
        // outermost, and layers must be applied before `add_routes`. The gate
        // sits inside it, so rejections are traced too. Health is not behind
        // a gate but is still traced.
        let router = server.layer(GrpcTraceLayer::new()).add_routes(routes);

        router
            .serve_with_incoming_shutdown(incoming.with_nodelay(Some(true)), signal)
            .await?;
        Ok(())
    }
}

/// Why a server failed to start or stopped abnormally.
///
/// `#[non_exhaustive]`: match with a wildcard arm.
#[derive(Debug)]
#[non_exhaustive]
pub enum ServeError {
    /// The listen address could not be bound.
    Bind(io::Error),
    /// The transport failed while serving.
    Transport(TransportError),
}

impl Error for ServeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Bind(source) => Some(source),
            Self::Transport(source) => Some(source),
        }
    }
}

impl From<TransportError> for ServeError {
    fn from(source: TransportError) -> Self {
        Self::Transport(source)
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

// Sets the overall service (`""`) and every added service to `status`.
async fn set_status(health: &HealthReporter, services: &[&'static str], status: ServingStatus) {
    health.set_service_status("", status).await;
    for service in services {
        health.set_service_status(service, status).await;
    }
}
