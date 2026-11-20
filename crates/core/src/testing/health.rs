//! Health-check queries for live-server tests.
//!
//! [`GrpcServer`](crate::server::GrpcServer) serves `grpc.health.v1.Health`
//! on its own, so a test only needs to ask. These helpers wrap the generated
//! health client, so tests need no `tonic-health` dependency.

use futures_util::{Stream, StreamExt};
use tonic::{Status, transport::Channel};
use tonic_health::pb::{HealthCheckRequest, health_client::HealthClient};

#[cfg(test)]
mod tests;

/// The status a health check reports: `Unknown`, `Serving`, `NotServing` or
/// `ServiceUnknown`.
///
/// This is the protobuf enum from `grpc.health.v1`.
pub use tonic_health::pb::health_check_response::ServingStatus;

/// Asks the server behind `channel` for the health of `service`, once, with no
/// token (health needs none).
///
/// Pass `""` for the server as a whole, or a service's `SERVICE_NAME` (the
/// const tonic generates in each `*_server` module) for one service. A
/// [`GrpcServer`](crate::server::GrpcServer) reports `Serving` for both once
/// it is running, and `NotServing` as soon as its shutdown signal fires.
///
/// # Errors
///
/// Returns the [`Status`] the call fails with: `NotFound` for a service the
/// server does not know, or a transport error as `Unavailable`.
///
/// ```no_run
/// # async fn run(channel: rust_grpc_lib::tonic::transport::Channel) {
/// use rust_grpc_lib::testing::{ServingStatus, health_status};
///
/// let status = health_status(channel, "").await.unwrap();
/// assert_eq!(status, ServingStatus::Serving);
/// # }
/// ```
pub async fn health_status(channel: Channel, service: &str) -> Result<ServingStatus, Status> {
    let response = HealthClient::new(channel)
        .check(HealthCheckRequest {
            service: service.to_owned(),
        })
        .await?;
    Ok(response.into_inner().status())
}

/// Opens a health `Watch` on `service` and returns the stream of its
/// statuses, with no token (health needs none).
///
/// `service` is as for [`health_status`]. The first item is the current
/// status, and each change follows. The stream does not end by itself: it ends
/// when the server stops or you drop it. Read it with
/// [`testing::StreamExt`](crate::testing::StreamExt), so no `futures-util`
/// dependency is needed.
///
/// A [`GrpcServer`](crate::server::GrpcServer) keeps `Watch` streams open
/// while it drains, so one reports `NotServing` when shutdown begins. Drop the
/// stream before waiting for the server to finish, or graceful shutdown waits
/// for it.
///
/// # Errors
///
/// The outer `Result` is the [`Status`] of opening the stream: `NotFound` for
/// a service the server does not know, as for [`health_status`]. Each item is
/// a `Result` too, since the stream can fail part-way, such as when the
/// connection drops.
///
/// ```no_run
/// # async fn run(channel: rust_grpc_lib::tonic::transport::Channel) {
/// use rust_grpc_lib::testing::{ServingStatus, StreamExt, health_watch};
///
/// let mut statuses = health_watch(channel, "").await.unwrap();
/// let first = statuses.next().await.unwrap().unwrap();
/// assert_eq!(first, ServingStatus::Serving);
/// # }
/// ```
pub async fn health_watch(
    channel: Channel,
    service: &str,
) -> Result<impl Stream<Item = Result<ServingStatus, Status>> + Send + Unpin + 'static, Status> {
    let response = HealthClient::new(channel)
        .watch(HealthCheckRequest {
            service: service.to_owned(),
        })
        .await?;
    Ok(response
        .into_inner()
        .map(|item| item.map(|response| response.status())))
}
