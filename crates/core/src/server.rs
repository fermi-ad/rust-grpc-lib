//! Serve gRPC services behind the access gate, with health checks, per-RPC
//! tracing, an optional timeout and graceful shutdown (`server` feature).
//!
//! [`GrpcServer::builder`] is the only way to serve services with this crate.
//! Each RPC passes through telemetry (outermost), tracing, then the service's
//! gate and generated server. Health has no gate. Use
//! [`without_telemetry`](GrpcServerBuilder::without_telemetry) to remove the
//! telemetry layer without changing tracing or access control.
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
//! # use rust_grpc_lib::__private::WireMethods;
//! # use rust_grpc_lib::auth::{ServicePolicy, TokenValidator};
//! # use rust_grpc_lib::server::{GrpcServer, GrpcService, ServeError};
//! # async fn run<V: TokenValidator, T: ServicePolicy<S>, S: GrpcService + WireMethods>(
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
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use opentelemetry::metrics::Meter;
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
use tower::{
    ServiceBuilder,
    util::{Either, MapResponseLayer},
};

use crate::__private::WireMethods;
use crate::auth::gate::AuthGate;
use crate::auth::{PolicyTable, ServicePolicy, TokenValidator};
use crate::telemetry::{
    CallSummary, Telemetry, TelemetryConfig,
    label::{LabelIndex, MethodLabels},
    layer::CallLayer,
    recorder::{OnCall, Recorder},
};
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
    ///
    /// No Tokio runtime is needed to build or get the meter. Automatic
    /// telemetry starts lazily on the first meter access or when serving.
    #[must_use = "a builder does nothing until services are added and `serve*` is called"]
    pub fn builder<V: TokenValidator>(validator: Arc<V>) -> GrpcServerBuilder<V> {
        let (reporter, health_service) = health_reporter();
        GrpcServerBuilder {
            validator,
            timeout: None,
            routes: Routes::new(health_service),
            health: reporter,
            served: Vec::new(),
            labels: LabelIndex::new(),
            telemetry: TelemetrySource::Auto(None),
            automatic_telemetry: OnceLock::new(),
            on_call: None,
        }
    }
}

/// Configures and runs a server; created by [`GrpcServer::builder`].
///
/// By default telemetry reads the environment. SDK disablement, no endpoint,
/// or exporter setup failure leaves call tracking on with a no-op meter.
/// When the server owns telemetry, it flushes after graceful drain or a
/// transport error, waiting up to five seconds. An outer timeout that drops
/// the serve future can prevent this awaited flush.
pub struct GrpcServerBuilder<V> {
    validator: Arc<V>,
    timeout: Option<Duration>,
    routes: Routes,
    health: HealthReporter,
    served: Vec<&'static str>,
    labels: LabelIndex,
    telemetry: TelemetrySource,
    automatic_telemetry: OnceLock<Telemetry>,
    on_call: Option<OnCall>,
}

enum TelemetrySource {
    Auto(Option<TelemetryConfig>),
    Shared(Telemetry),
    Off,
}

impl<V: TokenValidator> GrpcServerBuilder<V> {
    /// Use caller-owned telemetry. The server never shuts it down; the caller
    /// must call [`Telemetry::shutdown`] after all users have finished.
    ///
    /// The last telemetry selection wins. Select it before creating
    /// instruments with [`Self::meter`]; existing instruments are not moved.
    #[must_use = "this consumes the builder and returns an updated one; the telemetry selection is lost if the result is dropped"]
    pub fn telemetry(self, telemetry: &Telemetry) -> Self {
        self.with_telemetry_source(TelemetrySource::Shared(telemetry.clone()))
    }

    /// Set the resolved configuration for server-owned telemetry instead of
    /// using [`TelemetryConfig::from_env`] by default.
    ///
    /// The last telemetry selection wins. Select it before creating
    /// instruments with [`Self::meter`]; existing instruments are not moved.
    #[must_use = "this consumes the builder and returns an updated one; the telemetry config is lost if the result is dropped"]
    pub fn telemetry_config(self, config: TelemetryConfig) -> Self {
        self.with_telemetry_source(TelemetrySource::Auto(Some(config)))
    }

    /// Remove the telemetry layer, including call observation, and use a
    /// no-op meter. The last telemetry selection wins.
    #[must_use = "this consumes the builder and returns an updated one; the telemetry opt-out is lost if the result is dropped"]
    pub fn without_telemetry(self) -> Self {
        self.with_telemetry_source(TelemetrySource::Off)
    }

    /// Get the server's meter to create instruments before serving. No Tokio
    /// runtime is required. Returns a no-op meter when metrics are disabled.
    #[must_use = "use the returned meter to create instruments"]
    pub fn meter(&self) -> Meter {
        self.telemetry
            .resolve(&self.automatic_telemetry)
            .map_or_else(Telemetry::noop_meter, Telemetry::meter)
    }

    /// Observe each finished call, even with a no-op meter, unless the layer
    /// is removed with [`Self::without_telemetry`]. Replaces any prior observer.
    /// The callback runs on the serving path; keep it cheap and non-blocking.
    /// Panics are caught and logged without affecting the RPC.
    #[must_use = "this consumes the builder and returns an updated one; the call observer is lost if the result is dropped"]
    pub fn on_call(mut self, f: impl Fn(&CallSummary) + Send + Sync + 'static) -> Self {
        self.on_call = Some(Arc::new(f));
        self
    }

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
    /// The server type `S` also needs the wire method metadata emitted by
    /// `build_support`; regenerate older server code to get it.
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
        self.labels.register(MethodLabels {
            service_name: S::NAME,
            wire_methods: S::WIRE_METHODS,
            table,
        });
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

    fn with_telemetry_source(mut self, source: TelemetrySource) -> Self {
        self.telemetry = source;
        self.automatic_telemetry = OnceLock::new();
        self
    }

    #[cfg(test)]
    pub(crate) fn with_automatic_test_telemetry(mut self, telemetry: Telemetry) -> Self {
        self.telemetry = TelemetrySource::Auto(None);
        self.automatic_telemetry = OnceLock::from(telemetry);
        self
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
            automatic_telemetry,
            on_call,
            ..
        } = self;
        let metrics = telemetry.call_layer(&automatic_telemetry, labels, on_call);

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
        // First applied is outermost: telemetry -> trace -> routes/gate.
        // Both layers see gate rejections, unknown paths, and ungated health.
        // Normalize the different response bodies so both Either branches
        // have the same response type; the off branch does not track calls.
        let layer = match metrics {
            Some(metrics) => Either::Left(
                ServiceBuilder::new()
                    .map_response(|response: Response<_>| response.map(Body::new))
                    .layer(metrics)
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

        let result = router
            .serve_with_incoming_shutdown(incoming.with_nodelay(Some(true)), signal)
            .await;
        finish_serving(result, automatic_telemetry.into_inner()).await
    }
}

impl TelemetrySource {
    fn resolve<'a>(&'a self, automatic: &'a OnceLock<Telemetry>) -> Option<&'a Telemetry> {
        match self {
            Self::Auto(config) => Some(automatic.get_or_init(|| {
                config
                    .clone()
                    .map_or_else(Telemetry::from_env, Telemetry::start)
            })),
            Self::Shared(telemetry) => Some(telemetry),
            Self::Off => None,
        }
    }

    fn call_layer(
        &self,
        automatic: &OnceLock<Telemetry>,
        labels: LabelIndex,
        on_call: Option<OnCall>,
    ) -> Option<CallLayer<Recorder>> {
        self.resolve(automatic)
            .map(|telemetry| CallLayer::recording(telemetry, labels, on_call))
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

// Flush only the server-owned instance, regardless of the transport outcome.
async fn finish_serving(
    result: Result<(), TransportError>,
    automatic_telemetry: Option<Telemetry>,
) -> Result<(), TransportError> {
    if let Some(telemetry) = automatic_telemetry {
        telemetry.shutdown().await;
    }
    result
}

// Sets the overall service (`""`) and every added service to `status`.
async fn set_status(health: &HealthReporter, services: &[&'static str], status: ServingStatus) {
    health.set_service_status("", status).await;
    for service in services {
        health.set_service_status(service, status).await;
    }
}
