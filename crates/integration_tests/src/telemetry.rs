//! End-to-end server telemetry with generated routes, deterministic auth,
//! and isolated in-memory or loopback-only metrics collection.

use std::collections::BTreeMap;
use std::future::pending;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use rust_grpc_lib::access_controlled_service;
use rust_grpc_lib::auth::{KeyValidator, RequestContext};
use rust_grpc_lib::server::{GrpcServer, GrpcServerBuilder};
use rust_grpc_lib::telemetry::TelemetryConfig;
use rust_grpc_lib::testing::{
    InMemory, ServingStatus, StreamExt, TestServer, call_rpc, health_watch,
};
use rust_grpc_lib::tonic::{Code, Response, Status};
use tokio::sync::Notify;
use tokio::time::timeout;

use crate::services::example::{
    IdList, ReplyOne, ReplyTwo,
    example_db_client::ExampleDbClient,
    example_db_server::{ExampleDb, ExampleDbServer, SERVICE_NAME},
};
use crate::support::{
    Running, TEST_KID, TEST_USER, client, connect, health_status, make_jwt, make_validator, serve,
};

mod export;

const CALL: &str = "rpc.server.call.duration";
const ACTIVE: &str = "rpc.server.active_requests";
const CHECK: &str = "grpc.health.v1.Health/Check";
const WATCH: &str = "grpc.health.v1.Health/Watch";
const ONE: &str = "services.example.ExampleDB/getEndPointOne";
const TWO: &str = "services.example.ExampleDB/getEndPointTwo";
const DEADLINE: Duration = Duration::from_secs(5);

#[derive(Default)]
struct Stub {
    entered: Arc<Notify>,
}

#[access_controlled_service]
#[rust_grpc_lib::async_trait]
impl ExampleDb for Stub {
    #[roles(any("operator"))]
    async fn get_end_point_one(
        &self,
        _ctx: RequestContext<IdList>,
    ) -> Result<Response<ReplyOne>, Status> {
        Ok(Response::new(ReplyOne::default()))
    }

    #[any_authenticated]
    async fn get_end_point_two(
        &self,
        _ctx: RequestContext<IdList>,
    ) -> Result<Response<ReplyTwo>, Status> {
        self.entered.notify_one();
        pending().await
    }
}

fn builder() -> GrpcServerBuilder<KeyValidator> {
    GrpcServer::builder(make_validator())
}

async fn start(builder: GrpcServerBuilder<KeyValidator>) -> Running {
    serve(builder.add_service(ExampleDbServer::new, Stub::default())).await
}

async fn finish(running: Running) {
    running.shutdown.send(()).unwrap();
    timeout(DEADLINE, running.task)
        .await
        .expect("server drain and telemetry shutdown exceeded five seconds")
        .unwrap()
        .unwrap();
}

fn assert_calls(metrics: &InMemory, expected: &[(&str, Code, u64)]) {
    let mut actual = BTreeMap::new();
    for point in metrics.histograms(CALL) {
        assert_eq!(point.unit, "s");
        assert_eq!(point.bucket_counts.iter().sum::<u64>(), point.count);
        assert!(point.min.unwrap() >= 0.0);
        assert!(actual.insert(point.attributes, point.count).is_none());
    }
    let expected: BTreeMap<_, _> = expected
        .iter()
        .map(|&(method, code, count)| {
            let mut labels = BTreeMap::from([
                ("rpc.system.name".into(), "grpc".into()),
                ("rpc.method".into(), method.into()),
                ("rpc.response.status_code".into(), (code as i64).to_string()),
            ]);
            if code != Code::Ok {
                labels.insert("error.type".into(), format!("{code:?}"));
            }
            (labels, count)
        })
        .collect();
    assert_eq!(actual, expected);
    let active = metrics.counters(ACTIVE);
    assert!(!active.is_empty());
    assert!(active.iter().all(|point| point.value == 0.0), "{active:?}");
}

#[tokio::test]
async fn auth_health_and_unknown_routes_have_exact_bounded_labels() {
    let server = TestServer::builder(TEST_KID)
        .with_metrics()
        .add_service(ExampleDbServer::new, Stub::default())
        .start()
        .await;
    let metrics = server.metrics();
    let token = server.token(TEST_USER, &["operator"]);
    client(server.addr(), &token)
        .get_end_point_one(IdList::default())
        .await
        .unwrap();
    assert_eq!(
        client(server.addr(), &server.token(TEST_USER, &[]))
            .get_end_point_one(IdList::default())
            .await
            .unwrap_err()
            .code(),
        Code::PermissionDenied
    );
    let channel = connect(server.addr()).await;
    assert_eq!(
        ExampleDbClient::new(channel.clone())
            .get_end_point_one(IdList::default())
            .await
            .unwrap_err()
            .code(),
        Code::Unauthenticated
    );
    assert_eq!(
        health_status(server.addr(), SERVICE_NAME).await.unwrap(),
        ServingStatus::Serving
    );
    assert_eq!(
        call_rpc(
            channel.clone(),
            "/services.example.ExampleDB/GETENDPOINTONE",
            Some(&token)
        )
        .await
        .unwrap_err()
        .code(),
        Code::Unimplemented
    );
    const UNKNOWN_PATHS: u64 = 32;
    for index in 0..UNKNOWN_PATHS {
        assert_eq!(
            call_rpc(
                channel.clone(),
                &format!("/unknown.Service{index}/Method{index}"),
                None
            )
            .await
            .unwrap_err()
            .code(),
            Code::Unimplemented
        );
    }
    metrics
        .wait_until(
            |metrics| {
                metrics
                    .histograms(CALL)
                    .iter()
                    .map(|point| point.count)
                    .sum::<u64>()
                    >= UNKNOWN_PATHS + 6
            },
            DEADLINE,
        )
        .await;
    assert_calls(
        metrics,
        &[
            (ONE, Code::Ok, 1),
            (ONE, Code::PermissionDenied, 1),
            (ONE, Code::Unauthenticated, 1),
            (CHECK, Code::Ok, 2), // Includes the harness readiness check.
            ("_OTHER", Code::Unimplemented, UNKNOWN_PATHS + 1),
        ],
    );
    server.shutdown().await;
}

#[tokio::test]
async fn server_timeout_is_recorded_as_cancelled_once() {
    let server = TestServer::builder(TEST_KID)
        .with_metrics()
        .timeout(Duration::from_millis(100))
        .add_service(ExampleDbServer::new, Stub::default())
        .start()
        .await;
    let metrics = server.metrics();
    let error = timeout(
        DEADLINE,
        client(server.addr(), &server.token(TEST_USER, &[])).get_end_point_two(IdList::default()),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert_eq!(error.code(), Code::Cancelled);
    assert_eq!(metrics.calls(TWO, Code::Cancelled).await, 1);
    assert_calls(metrics, &[(CHECK, Code::Ok, 1), (TWO, Code::Cancelled, 1)]);
    server.shutdown().await;
}

#[tokio::test]
async fn grpc_timeout_is_recorded_as_cancelled_once() {
    let server = TestServer::builder(TEST_KID)
        .with_metrics()
        .add_service(ExampleDbServer::new, Stub::default())
        .start()
        .await;
    let metrics = server.metrics();
    let mut request = rust_grpc_lib::tonic::Request::new(IdList::default());
    request.set_timeout(Duration::from_millis(100));
    let err = timeout(
        DEADLINE,
        client(server.addr(), &server.token(TEST_USER, &[])).get_end_point_two(request),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert_eq!(err.code(), Code::Cancelled);
    assert_eq!(metrics.calls(TWO, Code::Cancelled).await, 1);
    assert_calls(metrics, &[(CHECK, Code::Ok, 1), (TWO, Code::Cancelled, 1)]);
    server.shutdown().await;
}

#[tokio::test]
async fn dropping_in_flight_unary_and_stream_records_cancelled_once_each() {
    let stub = Stub::default();
    let entered = Arc::clone(&stub.entered);
    let server = TestServer::builder(TEST_KID)
        .with_metrics()
        .add_service(ExampleDbServer::new, stub)
        .start()
        .await;
    let metrics = server.metrics();
    let mut client = client(server.addr(), &server.token(TEST_USER, &[]));
    let mut call = Box::pin(client.get_end_point_two(IdList::default()));
    timeout(DEADLINE, async {
        tokio::select! {
            () = entered.notified() => {},
            result = &mut call => panic!("pending RPC returned: {result:?}"),
        }
    })
    .await
    .expect("server did not enter unary handler");
    drop(call);
    assert_eq!(metrics.calls(TWO, Code::Cancelled).await, 1);

    let mut watch = health_watch(connect(server.addr()).await, "")
        .await
        .unwrap();
    assert_eq!(
        timeout(DEADLINE, watch.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        ServingStatus::Serving
    );
    assert!(
        !metrics
            .histograms(CALL)
            .iter()
            .any(|point| point.attributes["rpc.method"] == WATCH)
    );
    assert!(
        metrics
            .counters(ACTIVE)
            .iter()
            .any(|point| point.attributes["rpc.method"] == WATCH && point.value == 1.0)
    );
    drop(watch);
    assert_eq!(metrics.calls(WATCH, Code::Cancelled).await, 1);
    assert_calls(
        metrics,
        &[
            (CHECK, Code::Ok, 1),
            (TWO, Code::Cancelled, 1),
            (WATCH, Code::Cancelled, 1),
        ],
    );
    server.shutdown().await;
}

#[tokio::test]
async fn grace_expiry_closes_watch_before_final_metrics_snapshot() {
    let metrics = InMemory::new();
    let telemetry = metrics.telemetry();
    // Use the public builder: the harness intentionally aborts on its own
    // deadline rather than waiting for a forced close at the same deadline.
    let running = start(
        builder()
            .telemetry(&telemetry)
            .shutdown_grace(Duration::from_millis(150)),
    )
    .await;
    let mut watch = health_watch(connect(running.addr).await, "").await.unwrap();
    assert_eq!(
        timeout(DEADLINE, watch.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        ServingStatus::Serving
    );
    let shutdown = tokio::spawn(finish(running));
    assert_eq!(
        timeout(DEADLINE, watch.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        ServingStatus::NotServing
    );
    assert!(!shutdown.is_finished(), "watch should hold the drain open");
    assert!(!matches!(
        timeout(DEADLINE, watch.next()).await.unwrap(),
        Some(Ok(_))
    ));
    shutdown.await.unwrap();
    assert!(telemetry.is_enabled(), "the caller owns shared telemetry");
    telemetry.shutdown().await;
    assert_calls(&metrics, &[(WATCH, Code::Cancelled, 1)]);
}

#[tokio::test]
async fn panicking_observer_does_not_change_rpc_or_record_it_twice() {
    let observed = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&observed);
    let server = TestServer::builder(TEST_KID)
        .with_metrics()
        .on_call(move |summary| {
            if summary.method == Some(ONE) {
                count.fetch_add(1, Ordering::Relaxed);
                panic!("intentional observer panic");
            }
        })
        .add_service(ExampleDbServer::new, Stub::default())
        .start()
        .await;
    let metrics = server.metrics();
    client(server.addr(), &server.token(TEST_USER, &["operator"]))
        .get_end_point_one(IdList::default())
        .await
        .unwrap();
    assert_eq!(metrics.calls(ONE, Code::Ok).await, 1);
    assert_eq!(observed.load(Ordering::Relaxed), 1);
    assert_calls(metrics, &[(CHECK, Code::Ok, 1), (ONE, Code::Ok, 1)]);
    server.shutdown().await;
}
