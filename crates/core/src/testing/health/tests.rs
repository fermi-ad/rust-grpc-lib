//! Tests for [`health_status`] and [`health_watch`].
//!
//! Most run against a plain `tonic-health` server, where the test controls
//! each service's status. The shutdown test runs against a real
//! `GrpcServer`, which is what the helpers are for.

use std::{net::SocketAddr, time::Duration};

use tokio::{net::TcpListener, sync::oneshot, task::JoinHandle, time::timeout};
use tonic::transport::server::TcpIncoming;
use tonic_health::server::{HealthReporter, health_reporter};

use super::*;
use crate::server::GrpcServer;
use crate::testing::test_validator;

const WAIT: Duration = Duration::from_secs(5);

/// A server that stops when dropped.
struct Live {
    addr: SocketAddr,
    task: JoinHandle<()>,
}

impl Drop for Live {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Live {
    async fn channel(&self) -> Channel {
        Channel::from_shared(format!("http://{}", self.addr))
            .unwrap()
            .connect()
            .await
            .unwrap()
    }
}

/// Serves only the health service, and returns the reporter that controls it.
async fn serve_health_only() -> (Live, HealthReporter) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (reporter, service) = health_reporter();
    let task = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(service)
            .serve_with_incoming(TcpIncoming::from(listener))
            .await
            .unwrap();
    });
    (Live { addr, task }, reporter)
}

/// A `GrpcServer` with no services, and the sender of its shutdown signal.
async fn serve_grpc_server() -> (Live, oneshot::Sender<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (shutdown, signal) = oneshot::channel::<()>();
    let builder = GrpcServer::builder(test_validator("health-kid"));
    let task = tokio::spawn(async move {
        builder
            .serve_on_listener(listener, async {
                let _ = signal.await;
            })
            .await
            .unwrap();
    });
    (Live { addr, task }, shutdown)
}

async fn next_status(
    statuses: &mut (impl Stream<Item = Result<ServingStatus, Status>> + Unpin),
) -> ServingStatus {
    timeout(WAIT, statuses.next())
        .await
        .expect("a status must arrive")
        .expect("the stream must not end")
        .expect("the item must be a status")
}

#[tokio::test]
async fn status_of_the_whole_server_is_serving() {
    let (live, _reporter) = serve_health_only().await;

    let status = health_status(live.channel().await, "").await.unwrap();

    assert_eq!(status, ServingStatus::Serving);
}

#[tokio::test]
async fn status_of_a_named_service_follows_the_reporter() {
    let (live, reporter) = serve_health_only().await;
    reporter
        .set_service_status("demo.Greeter", tonic_health::ServingStatus::NotServing)
        .await;

    let status = health_status(live.channel().await, "demo.Greeter")
        .await
        .unwrap();

    assert_eq!(status, ServingStatus::NotServing);
}

#[tokio::test]
async fn status_of_an_unknown_service_is_not_found() {
    let (live, _reporter) = serve_health_only().await;

    let error = health_status(live.channel().await, "demo.Missing")
        .await
        .unwrap_err();

    assert_eq!(error.code(), tonic::Code::NotFound);
}

#[tokio::test]
async fn watch_starts_with_the_current_status() {
    let (live, _reporter) = serve_health_only().await;

    let mut statuses = health_watch(live.channel().await, "").await.unwrap();

    assert_eq!(next_status(&mut statuses).await, ServingStatus::Serving);
}

#[tokio::test]
async fn watch_yields_each_change() {
    let (live, reporter) = serve_health_only().await;
    reporter
        .set_service_status("demo.Greeter", tonic_health::ServingStatus::Serving)
        .await;
    let mut statuses = health_watch(live.channel().await, "demo.Greeter")
        .await
        .unwrap();
    assert_eq!(next_status(&mut statuses).await, ServingStatus::Serving);

    reporter
        .set_service_status("demo.Greeter", tonic_health::ServingStatus::NotServing)
        .await;
    assert_eq!(next_status(&mut statuses).await, ServingStatus::NotServing);

    reporter
        .set_service_status("demo.Greeter", tonic_health::ServingStatus::Serving)
        .await;
    assert_eq!(next_status(&mut statuses).await, ServingStatus::Serving);
}

#[tokio::test]
async fn watch_of_an_unknown_service_is_not_found() {
    let (live, _reporter) = serve_health_only().await;

    let error = health_watch(live.channel().await, "demo.Missing")
        .await
        .err()
        .expect("opening the watch must fail");

    assert_eq!(error.code(), tonic::Code::NotFound);
}

#[tokio::test]
async fn grpc_server_reports_serving_without_a_token() {
    let (live, _shutdown) = serve_grpc_server().await;

    let status = health_status(live.channel().await, "").await.unwrap();

    assert_eq!(status, ServingStatus::Serving);
}

#[tokio::test]
async fn grpc_server_watch_reports_not_serving_when_shutdown_begins() {
    let (live, shutdown) = serve_grpc_server().await;
    let mut statuses = health_watch(live.channel().await, "").await.unwrap();
    assert_eq!(next_status(&mut statuses).await, ServingStatus::Serving);

    shutdown.send(()).unwrap();

    assert_eq!(next_status(&mut statuses).await, ServingStatus::NotServing);
}

#[tokio::test]
async fn a_failed_connection_is_an_error() {
    // Bind to learn a free port, then close it so nothing listens there.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let channel = Channel::from_shared(format!("http://{addr}"))
        .unwrap()
        .connect_lazy();

    let error = health_status(channel, "").await.unwrap_err();

    assert_eq!(error.code(), tonic::Code::Unavailable);
}
