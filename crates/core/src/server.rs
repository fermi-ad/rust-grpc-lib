//! Serves gRPC services behind the access gate, with health checks, per-RPC
//! tracing and graceful shutdown (`server` feature). [`GrpcServer::builder`] is
//! the only way to serve services; auth cannot be switched off.
//!
//! Health reports `NotServing` as soon as shutdown fires, before the drain.
//!
//! ```rust,no_run
//! # use std::{sync::Arc, time::Duration};
//! # use rust_grpc_lib::__private::WireMethods;
//! # use rust_grpc_lib::auth::{ServicePolicy, TokenValidator};
//! # use rust_grpc_lib::server::{GrpcServer, GrpcService, ServeError};
//! # async fn run<V: TokenValidator, T: ServicePolicy<S>, S: GrpcService + WireMethods>(
//! #     validator: Arc<V>,
//! #     new_server: fn(T) -> S,
//! #     my_impl: T,
//! # ) -> Result<(), ServeError> {
//! // `new_server` is a generated constructor such as `FooServer::new`.
//! GrpcServer::builder(validator)
//!     .timeout(Duration::from_secs(30))
//!     .add_service(new_server, my_impl)
//!     .serve("0.0.0.0:50051".parse().unwrap())
//!     .await
//! # }
//! ```

use std::convert::Infallible;
use std::error::Error;
use std::fmt;
use std::future::{Future, pending};
use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use opentelemetry::metrics::Meter;
use tokio::{net::TcpListener, sync::oneshot, time::sleep};
use tonic::body::Body;
use tonic::codegen::Service;
use tonic::codegen::http::{Request, Response};
use tonic::server::NamedService;
use tonic::service::Routes;
use tonic::transport::server::TcpIncoming;
use tonic::transport::{Error as TransportError, Server};
use tonic_health::ServingStatus;
use tonic_health::server::{HealthReporter, health_reporter};
use tower::{
    ServiceBuilder,
    util::{Either, MapResponseLayer},
};

use crate::__private::WireMethods;
use crate::auth::gate::AuthGate;
use crate::auth::{PolicyTable, ServicePolicy, TokenValidator};
use crate::telemetry::{
    CallSummary, Telemetry, TelemetryConfig, label::MethodLabelIndex, layer::CallLayer,
    recorder::CallCompletionObserver,
};
use crate::trace::GrpcTraceLayer;

mod connections;
mod shutdown;
#[cfg(test)]
mod tests;

pub use shutdown::shutdown_signal;

const DEFAULT_SHUTDOWN_GRACE: Duration = Duration::from_secs(30);

/// Blanket-implemented; never implement it by hand.
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

/// Entry point for serving; start with [`GrpcServer::builder`].
#[derive(Debug, Clone, Copy)]
pub struct GrpcServer;

impl GrpcServer {
    /// Starts a builder that authenticates calls with `validator`.
    ///
    /// ```
    /// # use std::sync::Arc;
    /// # use rust_grpc_lib::{auth::TokenValidator, server::GrpcServer};
    /// # fn run<V: TokenValidator>(validator: Arc<V>) {
    /// let builder = GrpcServer::builder(validator);
    /// # }
    /// ```
    #[must_use = "a builder does nothing until services are added and `serve*` is called"]
    pub fn builder<V: TokenValidator>(validator: Arc<V>) -> GrpcServerBuilder<V> {
        let (reporter, health_service) = health_reporter();
        GrpcServerBuilder {
            validator,
            timeout: None,
            routes: Routes::new(health_service),
            health: reporter,
            served: Vec::new(),
            labels: MethodLabelIndex::new(),
            telemetry: TelemetrySource::ServerOwned(None, OnceLock::new()),
            on_call: None,
            handle_signals: true,
            shutdown_grace: DEFAULT_SHUTDOWN_GRACE,
        }
    }
}

/// Automatic telemetry reads the environment on first meter access or when
/// serving. If export is unavailable, call tracking still runs with a no-op meter.
///
/// The server flushes its own provider after draining or a transport failure.
/// Dropping the serve future closes connections but can interrupt that flush.
pub struct GrpcServerBuilder<V> {
    validator: Arc<V>,
    timeout: Option<Duration>,
    routes: Routes,
    health: HealthReporter,
    served: Vec<&'static str>,
    labels: MethodLabelIndex,
    telemetry: TelemetrySource,
    on_call: Option<CallCompletionObserver>,
    handle_signals: bool,
    shutdown_grace: Duration,
}

enum TelemetrySource {
    ServerOwned(Option<TelemetryConfig>, OnceLock<Telemetry>),
    CallerOwned(Telemetry),
    Off,
}

impl<V: TokenValidator> GrpcServerBuilder<V> {
    /// The server never shuts caller-owned telemetry down; call
    /// [`Telemetry::shutdown`] after all users have finished. See [`Self::meter`].
    #[must_use = "this consumes the builder and returns an updated one; the telemetry selection is lost if the result is dropped"]
    pub fn with_caller_owned_telemetry(mut self, telemetry: &Telemetry) -> Self {
        self.telemetry = TelemetrySource::CallerOwned(telemetry.clone());
        self
    }

    /// Overrides [`TelemetryConfig::from_env`] for server-owned telemetry.
    #[must_use = "this consumes the builder and returns an updated one; the telemetry config is lost if the result is dropped"]
    pub fn telemetry_config(mut self, config: TelemetryConfig) -> Self {
        self.telemetry = TelemetrySource::ServerOwned(Some(config), OnceLock::new());
        self
    }

    /// Removes the telemetry layer, including call observation. The last
    /// telemetry selection wins.
    #[must_use = "this consumes the builder and returns an updated one; the telemetry opt-out is lost if the result is dropped"]
    pub fn without_telemetry(mut self) -> Self {
        self.telemetry = TelemetrySource::Off;
        self
    }

    /// Works without a Tokio runtime; no-op if metrics are disabled. Existing
    /// instruments stay on their original meter when the selection changes.
    #[must_use = "use the returned meter to create instruments"]
    pub fn meter(&self) -> Meter {
        self.telemetry
            .resolve()
            .map_or_else(Telemetry::noop_meter, Telemetry::meter)
    }

    /// Runs once per call, including cancellation, after metrics are recorded
    /// (even with a no-op meter) unless [`Self::without_telemetry`] is set. Keep
    /// it non-blocking; panics are caught and logged without changing the outcome.
    #[must_use = "this consumes the builder and returns an updated one; the call observer is lost if the result is dropped"]
    pub fn on_call_complete(mut self, f: impl Fn(&CallSummary) + Send + Sync + 'static) -> Self {
        self.on_call = Some(Arc::new(f));
        self
    }

    /// The handler is dropped at the deadline and the client gets `Cancelled`.
    /// A shorter client `grpc-timeout` still wins. Default: none.
    #[must_use = "this consumes the builder and returns an updated one; the timeout is lost if the result is dropped"]
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// `T: ServicePolicy<S>` is unsatisfied if `T`'s `impl` block lacks
    /// [`#[access_controlled_service]`](crate::access_controlled_service), or
    /// if `constructor` belongs to a different service. `S` also needs the wire
    /// method metadata emitted by `build_support`; regenerate older server code.
    #[must_use = "this consumes the builder and returns an updated one; the service is lost if the result is dropped"]
    pub fn add_service<T, S>(mut self, constructor: impl FnOnce(T) -> S, implementation: T) -> Self
    where
        T: ServicePolicy<S>,
        S: GrpcService + WireMethods,
    {
        let table = PolicyTable::of::<S, T>();
        let gate = AuthGate::new(
            constructor(implementation),
            table,
            Arc::clone(&self.validator),
        );
        self.routes = self.routes.add_service(gate);
        self.served.push(S::NAME);
        self.labels.register(S::NAME, S::WIRE_METHODS, table);
        self
    }

    /// Drain limit (default: 30 seconds), counted from when health becomes
    /// `NotServing`; zero closes connections immediately. Remaining RPCs are
    /// dropped at expiry, which is not an error. The server-owned telemetry
    /// flush can take up to five more seconds.
    #[must_use = "this consumes the builder and returns an updated one; the shutdown grace is lost if the result is dropped"]
    pub fn shutdown_grace(mut self, grace: Duration) -> Self {
        self.shutdown_grace = grace;
        self
    }

    /// Only affects [`Self::serve`], which then runs until it fails or its
    /// future is dropped.
    #[must_use = "this consumes the builder and returns an updated one; the signal opt-out is lost if the result is dropped"]
    pub fn without_signal_handling(mut self) -> Self {
        self.handle_signals = false;
        self
    }

    /// Serves on `addr`, draining on SIGTERM or SIGINT (Unix) or Ctrl-C
    /// (Windows). Handlers are registered before binding; see [`shutdown_signal`]
    /// for the process-wide effects, and [`Self::without_signal_handling`] to opt out.
    ///
    /// # Errors
    ///
    /// [`ServeError::Signal`] if handlers cannot be registered,
    /// [`ServeError::Bind`] if `addr` cannot be bound, [`ServeError::Transport`]
    /// if the transport fails.
    ///
    /// # Panics
    ///
    /// Requires a Tokio runtime with I/O enabled (`enable_all()` is sufficient).
    pub async fn serve(self, addr: SocketAddr) -> Result<(), ServeError> {
        if self.handle_signals {
            let signal = shutdown::register().map_err(ServeError::Signal)?;
            self.serve_with_shutdown(addr, signal).await
        } else {
            self.serve_with_shutdown(addr, pending()).await
        }
    }

    /// Only `shutdown` triggers the drain; no process handlers are registered
    /// (see [`shutdown_signal`]). Health flips to `NotServing` first so load
    /// balancers stop sending traffic. Open streams, including health `Watch`,
    /// are closed if they outlast [`Self::shutdown_grace`].
    ///
    /// # Errors
    ///
    /// [`ServeError::Bind`] or [`ServeError::Transport`]; a completed shutdown
    /// returns `Ok(())`.
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
    /// already bound (in tests, bind port `0`). Never returns [`ServeError::Bind`].
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
            labels,
            telemetry,
            on_call,
            shutdown_grace,
            ..
        } = self;
        let call_telemetry = telemetry.resolve();
        let (call_drain, call_drain_guard) = tokio::sync::watch::channel(());
        let call_layer = call_telemetry
            .map(move |telemetry| CallLayer::new(telemetry, labels, on_call, call_drain_guard));

        set_status(&health, &served, ServingStatus::Serving).await;
        let mut probe_guard = call_telemetry.and_then(Telemetry::attach_runtime_probe);
        // Flip health before the graceful drain begins so probes see
        // `NotServing` while in-flight RPCs finish.
        let (start_grace, grace_started) = oneshot::channel();
        let probe_guard_on_shutdown = &mut probe_guard;
        let signal = async move {
            shutdown.await;
            drop(probe_guard_on_shutdown.take());
            set_status(&health, &served, ServingStatus::NotServing).await;
            let _ = start_grace.send(());
        };

        let mut server = Server::builder();
        if let Some(timeout) = timeout {
            server = server.timeout(timeout);
        }
        // Both layers wrap routing, so they see gate rejections, unknown paths,
        // and ungated health. Either requires matching response body types.
        let layer = match call_layer {
            Some(call_layer) => Either::Left(
                ServiceBuilder::new()
                    .map_response(|response: Response<_>| response.map(Body::new))
                    .layer(call_layer)
                    .into_inner(),
            ),
            None => Either::Right(MapResponseLayer::new(|response: Response<_>| {
                response.map(Body::new)
            })),
        };
        let router = server
            .layer(layer)
            .layer(GrpcTraceLayer::new())
            .add_routes(routes);

        let connections = connections::Connections::default();
        let result = {
            let serve = router.serve_with_incoming_shutdown(connections.incoming(incoming), signal);
            tokio::pin!(serve);
            tokio::select! {
                biased;
                result = &mut serve => result,
                () = async {
                    if grace_started.await.is_ok() {
                        sleep(shutdown_grace).await;
                    } else {
                        pending::<()>().await;
                    }
                } => {
                    tracing::warn!(grace = ?shutdown_grace, "shutdown grace expired; closing remaining connections");
                    connections.close();
                    // Keep tonic's join for its connection tasks. Dropping
                    // the serve future by itself would leave them running.
                    serve.await
                }
            }
        };
        drop(connections);
        // A transport error can bypass the signal's probe cleanup.
        drop(probe_guard);
        // Hyper can run HTTP/2 streams on separate tasks. Before flushing,
        // let those tasks release their services/bodies and record cancellation.
        call_drain.closed().await;
        let server_owned_telemetry = match telemetry {
            TelemetrySource::ServerOwned(_, cached) => cached.into_inner(),
            _ => None,
        };
        if let Some(telemetry) = server_owned_telemetry {
            telemetry.shutdown().await;
        }
        result
    }
}

impl TelemetrySource {
    fn resolve(&self) -> Option<&Telemetry> {
        match self {
            Self::ServerOwned(config, cached) => Some(cached.get_or_init(|| {
                Telemetry::start(config.clone().unwrap_or_else(TelemetryConfig::from_env))
            })),
            Self::CallerOwned(telemetry) => Some(telemetry),
            Self::Off => None,
        }
    }
}

/// Only [`GrpcServerBuilder::serve`] can return [`Self::Signal`].
#[derive(Debug)]
#[non_exhaustive]
pub enum ServeError {
    /// The listen address could not be bound.
    Bind(io::Error),
    /// The shutdown signal handlers could not be registered.
    Signal(io::Error),
    /// The transport failed while serving.
    Transport(TransportError),
}

impl Error for ServeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Bind(source) | Self::Signal(source) => Some(source),
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
            Self::Signal(source) => {
                write!(
                    f,
                    "failed to register the shutdown signal handlers: {source}"
                )
            }
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
