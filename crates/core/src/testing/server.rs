//! Isolated loopback servers for tests of gRPC services.

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep_until, timeout};
use tonic::transport::Endpoint;

use crate::__private::WireMethods;
use crate::auth::{KeyValidator, ServicePolicy};
use crate::server::{GrpcServer, GrpcServerBuilder, GrpcService, ServeError};
use crate::telemetry::TelemetryConfig;

use super::{InMemoryMetrics, ServingStatus, health_status, test_token, test_validator};

#[cfg(test)]
mod tests;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// Only for tests: the validator trusts a publicly known signing key. No
/// process signal handlers or environment-configured exporters are installed.
/// Prefer [`Self::shutdown`] to assert graceful shutdown and flush metrics;
/// dropping the handle aborts the server without waiting.
///
/// ```no_run
/// use rust_grpc_lib::testing::TestServer;
///
/// # async fn run() {
/// let server = TestServer::builder("test-kid")
///     // Add generated services with .add_service(FooServer::new, implementation).
///     .with_metrics()
///     .start().await;
/// let url = server.url();
/// let token = server.token("alice", &["operator"]);
/// // Connect the generated client to url and send token as a bearer token.
/// let _ = server.metrics();
/// server.shutdown().await;
/// # }
/// ```
#[must_use = "dropping the handle aborts the test server"]
pub struct TestServer {
    kid: String,
    addr: SocketAddr,
    url: String,
    metrics: Option<InMemoryMetrics>,
    shutdown_timeout: Duration,
    shutdown: Option<oneshot::Sender<()>>,
    task: JoinHandle<Result<(), ServeError>>,
}

/// Created by [`TestServer::builder`]; call [`Self::start`] to run it.
#[must_use = "the builder must be started to run the test server"]
pub struct TestServerBuilder {
    kid: String,
    builder: GrpcServerBuilder<KeyValidator>,
    metrics: Option<InMemoryMetrics>,
    shutdown_timeout: Duration,
}

impl TestServer {
    /// Accepts tokens minted by [`Self::token`], signed under `kid`.
    pub fn builder(kid: &str) -> TestServerBuilder {
        TestServerBuilder {
            kid: kid.to_owned(),
            builder: GrpcServer::builder(test_validator(kid))
                .telemetry_config(TelemetryConfig::default()),
            metrics: None,
            shutdown_timeout: DEFAULT_SHUTDOWN_TIMEOUT,
        }
    }

    /// The IPv4 loopback address the server is bound to.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The `http://` URL to connect a generated client to.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// A bearer token for `sub` holding the given realm roles.
    #[must_use = "the token is returned, not sent anywhere"]
    pub fn token(&self, sub: &str, roles: &[&str]) -> String {
        test_token(&self.kid, sub, roles)
    }

    /// # Panics
    ///
    /// If [`TestServerBuilder::with_metrics`] was not selected.
    pub fn metrics(&self) -> &InMemoryMetrics {
        self.metrics
            .as_ref()
            .expect("TestServer metrics require .with_metrics()")
    }

    /// Drains gracefully, then flushes the in-memory provider, if any.
    ///
    /// # Panics
    ///
    /// If the server fails or does not drain within the configured
    /// [`TestServerBuilder::shutdown_timeout`]. Drop open streams before
    /// awaiting shutdown unless they finish on their own.
    pub async fn shutdown(mut self) {
        let deadline = Instant::now() + self.shutdown_timeout;
        // Start the harness deadline first and let it win ties, so a forced
        // close cannot pass as a graceful drain.
        let _ = self.shutdown.take().unwrap().send(());
        let result = tokio::select! {
            biased;
            () = sleep_until(deadline) => {
                self.task.abort();
                let _ = (&mut self.task).await;
                None
            }
            result = &mut self.task => Some(result),
        };
        if let Some(metrics) = &self.metrics {
            metrics.telemetry().shutdown().await;
        }
        match result {
            Some(Ok(Ok(()))) => {}
            Some(Ok(Err(error))) => panic!("TestServer at {} failed: {error}", self.url),
            Some(Err(error)) => panic!("TestServer at {} task failed: {error}", self.url),
            None => panic!(
                "TestServer at {} did not shut down gracefully within {:?}; check for open RPC streams",
                self.url, self.shutdown_timeout
            ),
        }
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl TestServerBuilder {
    /// Like [`GrpcServerBuilder::add_service`]: pass the generated constructor
    /// and your implementation, e.g. `.add_service(FooServer::new, MyFoo)`.
    pub fn add_service<T, S>(mut self, constructor: impl FnOnce(T) -> S, implementation: T) -> Self
    where
        T: ServicePolicy<S>,
        S: GrpcService + WireMethods,
    {
        self.builder = self.builder.add_service(constructor, implementation);
        self
    }

    /// Uses an isolated [`InMemoryMetrics`] backend regardless of environment settings.
    pub fn with_metrics(mut self) -> Self {
        let metrics = InMemoryMetrics::new();
        self.builder = self
            .builder
            .with_caller_owned_telemetry(&metrics.telemetry());
        self.metrics = Some(metrics);
        self
    }

    /// Like [`GrpcServerBuilder::timeout`].
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.builder = self.builder.timeout(timeout);
        self
    }

    /// Bounds the graceful drain (default: five seconds).
    pub fn shutdown_timeout(mut self, timeout: Duration) -> Self {
        self.shutdown_timeout = timeout;
        self
    }

    /// Returns once the server reports `Serving`; that readiness check is
    /// recorded if metrics are enabled.
    ///
    /// # Panics
    ///
    /// If binding fails or the server does not become healthy within five
    /// seconds. Requires a Tokio runtime with I/O and time enabled.
    pub async fn start(self) -> TestServer {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("TestServer could not bind a loopback listener");
        let addr = listener.local_addr().expect("TestServer listener address");
        let (shutdown, signal) = oneshot::channel();
        let builder = self.builder.shutdown_grace(self.shutdown_timeout);
        let task = tokio::spawn(builder.serve_on_listener(listener, async {
            let _ = signal.await;
        }));
        // Guard the task before awaiting readiness so a canceled start aborts it.
        let mut server = TestServer {
            kid: self.kid,
            addr,
            url: format!("http://{addr}"),
            metrics: self.metrics,
            shutdown_timeout: self.shutdown_timeout,
            shutdown: Some(shutdown),
            task,
        };
        let endpoint = Endpoint::from_shared(server.url.clone()).expect("valid TestServer URL");
        let ready = timeout(STARTUP_TIMEOUT, async {
            let channel = endpoint
                .connect()
                .await
                .map_err(|error| error.to_string())?;
            health_status(channel, "")
                .await
                .map_err(|error| error.to_string())
        });
        tokio::select! {
            biased;
            result = &mut server.task => panic!("TestServer at {} stopped before readiness: {result:?}", server.url),
            result = ready => match result {
                Ok(Ok(ServingStatus::Serving)) => {},
                Ok(Ok(status)) => panic!("TestServer at {} is not serving: {status:?}", server.url),
                Ok(Err(error)) => panic!("TestServer at {} readiness check failed: {error}", server.url),
                Err(_) => panic!("TestServer at {} was not ready within {STARTUP_TIMEOUT:?}", server.url),
            }
        }
        server
    }
}
