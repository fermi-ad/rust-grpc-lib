//! Focused coverage of the test harness itself.

use std::convert::Infallible;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::task::{Context, Poll};

use tokio::net::TcpStream;
use tonic::body::Body;
use tonic::codegen::{BoxFuture, Service, http};
use tonic::server::NamedService;
use tonic::{Code, Request, Response, Status};
use tonic_prost::ProstCodec;

use crate::auth::{Access, MethodPolicy};
use crate::testing::{StreamExt, call_rpc, health_watch};

use super::*;

const KID: &str = "test-server-kid";
const METHOD: &str = "test.Harness/Ping";
const CALL_DURATION: &str = "rpc.server.call.duration";

#[derive(Clone)]
struct Stub;

impl NamedService for Stub {
    const NAME: &'static str = "test.Harness";
}

impl WireMethods for Stub {
    const WIRE_METHODS: &'static [&'static str] = &[METHOD];
}

impl ServicePolicy<Stub> for Stub {
    const POLICIES: &'static [MethodPolicy] = &[MethodPolicy::new("ping", Access::Authenticated)];
}

impl Service<http::Request<Body>> for Stub {
    type Response = http::Response<Body>;
    type Error = Infallible;
    type Future = BoxFuture<Self::Response, Self::Error>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: http::Request<Body>) -> Self::Future {
        struct Ping;

        impl tonic::server::UnaryService<()> for Ping {
            type Response = ();
            type Future = std::future::Ready<Result<Response<()>, Status>>;

            fn call(&mut self, _: Request<()>) -> Self::Future {
                std::future::ready(Ok(Response::new(())))
            }
        }

        Box::pin(async move {
            Ok(tonic::server::Grpc::new(ProstCodec::default())
                .unary(Ping, request)
                .await)
        })
    }
}

#[tokio::test]
async fn authenticated_stub_records_isolated_metrics_and_shutdown_stops_provider() {
    let server = TestServer::builder(KID)
        .add_service(|stub| stub, Stub)
        .with_metrics()
        .start()
        .await;
    let other = TestServer::builder("other-kid")
        .with_metrics()
        .start()
        .await;
    assert_eq!(server.addr().ip(), Ipv4Addr::LOCALHOST);
    assert_ne!(server.addr(), other.addr());

    // The readiness check passed through the new server's own telemetry.
    assert_eq!(
        server
            .metrics()
            .calls("grpc.health.v1.Health/Check", Code::Ok)
            .await,
        1
    );
    let channel = Endpoint::new(server.url().to_owned())
        .unwrap()
        .connect()
        .await
        .unwrap();
    assert_eq!(
        health_status(channel.clone(), Stub::NAME).await.unwrap(),
        ServingStatus::Serving
    );
    let token = server.token("alice", &["operator"]);
    call_rpc(channel, &format!("/{METHOD}"), Some(&token))
        .await
        .unwrap();
    assert_eq!(server.metrics().calls(METHOD, Code::Ok).await, 1);
    assert!(
        other
            .metrics()
            .histograms(CALL_DURATION)
            .iter()
            .all(|point| point.attributes["rpc.method"] != METHOD)
    );

    let addr = server.addr();
    let telemetry = server.metrics().telemetry();
    // The provider clone remains alive, so the harness must explicitly stop it.
    assert!(telemetry.is_enabled());
    server.shutdown().await;
    assert!(!telemetry.is_enabled());
    assert!(TcpStream::connect(addr).await.is_err());
    other.shutdown().await;
}

#[tokio::test]
async fn without_telemetry_overrides_metrics_and_disables_observer() {
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let server = TestServer::builder(KID)
        .with_metrics()
        .on_call(move |_| {
            observed.fetch_add(1, Ordering::Relaxed);
        })
        .without_telemetry()
        .start()
        .await;
    assert!(server.metrics.is_none());
    server.shutdown().await;
    assert_eq!(calls.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn shutdown_fails_and_cleans_up_if_a_stream_stays_open() {
    let server = TestServer::builder(KID)
        .shutdown_timeout(Duration::from_millis(100))
        .with_metrics()
        .start()
        .await;
    let addr = server.addr();
    let telemetry = server.metrics().telemetry();
    let channel = Endpoint::new(server.url().to_owned())
        .unwrap()
        .connect()
        .await
        .unwrap();
    let mut stream = health_watch(channel, "").await.unwrap();
    assert_eq!(
        stream.next().await.unwrap().unwrap(),
        ServingStatus::Serving
    );

    let error = timeout(Duration::from_secs(5), tokio::spawn(server.shutdown()))
        .await
        .unwrap()
        .unwrap_err();
    let panic = error.into_panic();
    assert!(
        panic
            .downcast_ref::<String>()
            .unwrap()
            .contains("did not shut down gracefully within 100ms")
    );
    assert!(!telemetry.is_enabled());
    assert!(TcpStream::connect(addr).await.is_err());
    // The existing watch must also be closed, not just the listening socket.
    timeout(Duration::from_secs(5), async {
        while let Some(Ok(_)) = stream.next().await {}
    })
    .await
    .expect("shutdown timeout must close the open stream");
}

#[tokio::test]
async fn dropping_the_handle_aborts_the_server() {
    let server = TestServer::builder(KID).start().await;
    let addr = server.addr();
    let task = server.task.abort_handle();
    drop(server);
    timeout(Duration::from_secs(5), async {
        while !task.is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("dropping the harness must stop the server task");
    assert!(TcpStream::connect(addr).await.is_err());
}
