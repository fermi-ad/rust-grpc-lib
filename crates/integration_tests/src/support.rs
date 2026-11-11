//! Helpers shared by the live-server tests. Servers bind `127.0.0.1:0`, so
//! tests get their own port and run in parallel.

use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tracing::Level;
use tracing::subscriber::{DefaultGuard, set_default};
use tracing_subscriber::fmt::MakeWriter;

use rust_grpc_lib::auth::{AuthenticatedClient, ForwardedToken, KeyValidator, TokenValidator};
use rust_grpc_lib::server::{GrpcServerBuilder, ServeError};
use rust_grpc_lib::testing::{ServingStatus, SpanCapture, SpanRecord, test_token, test_validator};
use rust_grpc_lib::tonic::transport::Channel;

use crate::services::example::example_db_client::ExampleDbClient;

/// `kid` of the test signing key.
pub const TEST_KID: &str = "integration-test-kid";

/// `sub` of every [`make_jwt`] token.
pub const TEST_USER: &str = "test-user";

/// The client type returned by [`client`].
pub type Client = ExampleDbClient<AuthenticatedClient<ForwardedToken>>;

/// RS256 token for [`TEST_USER`] with these realm roles, valid for an hour.
pub fn make_jwt(roles: &[&str]) -> String {
    test_token(TEST_KID, TEST_USER, roles)
}

/// Validator accepting tokens signed under [`TEST_KID`].
pub fn make_validator() -> Arc<KeyValidator> {
    test_validator(TEST_KID)
}

/// A server started by [`serve`]. Dropping it (or `shutdown`) starts
/// graceful shutdown, so keep it alive for the whole test.
pub struct Running {
    pub addr: SocketAddr,
    pub shutdown: oneshot::Sender<()>,
    pub task: JoinHandle<Result<(), ServeError>>,
}

/// Serves `builder` on a fresh loopback port until [`Running::shutdown`]
/// fires or is dropped.
pub async fn serve<V: TokenValidator>(builder: GrpcServerBuilder<V>) -> Running {
    // Bound before spawning, so early connections queue in the backlog and
    // no startup delay is needed.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (shutdown, rx) = oneshot::channel::<()>();
    let task = tokio::spawn(async move {
        builder
            .serve_on_listener(listener, async {
                let _ = rx.await;
            })
            .await
    });
    Running {
        addr,
        shutdown,
        task,
    }
}

/// Client for `addr` sending `token` as its bearer token.
pub fn client(addr: SocketAddr, token: &str) -> Client {
    ExampleDbClient::authenticated(
        &format!("http://{addr}"),
        ForwardedToken::new(token.to_string()),
    )
    .unwrap()
}

/// Connected channel to `addr` that sends no token.
pub async fn connect(addr: SocketAddr) -> Channel {
    Channel::from_shared(format!("http://{addr}"))
        .unwrap()
        .connect()
        .await
        .unwrap()
}

/// Health `Check` for `service` (`""` = whole server), without a token.
pub async fn health_status(
    addr: SocketAddr,
    service: &str,
) -> Result<ServingStatus, rust_grpc_lib::tonic::Status> {
    rust_grpc_lib::testing::health_status(connect(addr).await, service).await
}

/// The one `rpc` span with this `rpc.method`; panics on zero or several.
pub fn rpc_span(capture: &SpanCapture, method: &str) -> SpanRecord {
    let mut matching: Vec<_> = capture
        .spans("rpc")
        .into_iter()
        .filter(|span| span.field("rpc.method") == Some(method))
        .collect();
    assert_eq!(matching.len(), 1, "expected one rpc span for {method}");
    matching.remove(0)
}

/// Text sink for [`capture_logs`].
#[derive(Clone, Default)]
pub struct Capture(Arc<Mutex<Vec<u8>>>);

impl Capture {
    /// Everything written so far.
    pub fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

impl io::Write for Capture {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Capture {
    type Writer = Capture;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Captures formatted events at every level while the guard lives. The
/// subscriber is thread-local, so the test must use the current-thread
/// runtime (the `#[tokio::test]` default) to see the server task's events.
pub fn capture_logs() -> (Capture, DefaultGuard) {
    let capture = Capture::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(capture.clone())
        .with_ansi(false)
        .with_max_level(Level::TRACE)
        .finish();
    let guard = set_default(subscriber);
    (capture, guard)
}
