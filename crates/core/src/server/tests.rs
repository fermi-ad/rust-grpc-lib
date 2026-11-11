//! Tests for the server builder that need no running server. The live-server
//! behavior is covered in the `integration_tests` crate.

use super::*;

use tonic_health::pb::HealthCheckRequest;
use tonic_health::pb::health_check_response::ServingStatus as WireStatus;
use tonic_health::pb::health_server::Health;
use tonic_health::server::HealthService;

/// Read a service's status through the public `Health` service.
async fn status_of(reporter: &HealthReporter, name: &str) -> WireStatus {
    HealthService::from_health_reporter(reporter.clone())
        .check(tonic::Request::new(HealthCheckRequest {
            service: name.to_string(),
        }))
        .await
        .unwrap()
        .into_inner()
        .status()
}

#[tokio::test]
async fn set_status_updates_the_overall_service_and_every_listed_service() {
    let (reporter, _service) = health_reporter();

    set_status(&reporter, &["a.A", "b.B"], ServingStatus::NotServing).await;

    for name in ["", "a.A", "b.B"] {
        assert_eq!(
            status_of(&reporter, name).await,
            WireStatus::NotServing,
            "service {name:?}"
        );
    }
}
