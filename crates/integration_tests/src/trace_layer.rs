//! `GrpcTraceLayer` in a full server stack: `rpc` span fields, trailer
//! status, the `RpcMethod` extension, health and `Cancelled` on timeout.
//! Field semantics: `rust_grpc_lib::server`. Current-thread runtime
//! throughout, so the thread-local [`capture_spans`] subscriber sees the
//! server.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::time::sleep;

use rust_grpc_lib::access_controlled_service;
use rust_grpc_lib::auth::{RequestContext, RpcMethod};
use rust_grpc_lib::server::GrpcServer;
use rust_grpc_lib::testing::capture_spans;
use rust_grpc_lib::tonic::{Code, Response, Status};

use crate::services::example::{
    IdList, ReplyOne, ReplyTwo,
    example_db_server::{ExampleDb, ExampleDbServer, SERVICE_NAME},
};
use crate::support::{
    Running, TEST_USER, client, health_status, make_jwt, make_validator, rpc_span, serve,
};

/// `getEndPointOne` runs in its own `#[instrument]` span and records the
/// `RpcMethod` it saw.
struct Stub {
    seen_method: Arc<Mutex<Option<RpcMethod>>>,
}

#[access_controlled_service]
#[rust_grpc_lib::async_trait]
impl ExampleDb for Stub {
    #[any_authenticated]
    #[tracing::instrument(skip_all)]
    async fn get_end_point_one(
        &self,
        ctx: RequestContext<IdList>,
    ) -> Result<Response<ReplyOne>, Status> {
        *self.seen_method.lock().unwrap() = Some(ctx.rpc.clone());
        Ok(Response::new(ReplyOne {
            section_one: vec![],
        }))
    }

    /// Sleeps 30 s, to trip the timeout.
    #[any_authenticated]
    async fn get_end_point_two(
        &self,
        _ctx: RequestContext<IdList>,
    ) -> Result<Response<ReplyTwo>, Status> {
        sleep(Duration::from_secs(30)).await;
        Err(Status::unimplemented("unreachable in the tests"))
    }
}

struct Started {
    running: Running,
    seen_method: Arc<Mutex<Option<RpcMethod>>>,
}

async fn start(timeout: Option<Duration>) -> Started {
    let seen_method = Arc::new(Mutex::new(None));
    let mut builder = GrpcServer::builder(make_validator());
    if let Some(timeout) = timeout {
        builder = builder.timeout(timeout);
    }
    let running = serve(builder.add_service(
        ExampleDbServer::new,
        Stub {
            seen_method: Arc::clone(&seen_method),
        },
    ))
    .await;
    Started {
        running,
        seen_method,
    }
}

#[tokio::test]
async fn unary_rpc_gets_a_span_with_fields_and_final_status() {
    let (capture, _guard) = capture_spans();
    let started = start(None).await;

    client(started.running.addr, &make_jwt(&[]))
        .get_end_point_one(IdList { id: vec![1] })
        .await
        .expect("a valid token reaches the handler");

    let span = rpc_span(&capture, "getEndPointOne");
    assert_eq!(span.field("rpc.system"), Some("grpc"));
    assert_eq!(span.field("rpc.service"), Some(SERVICE_NAME));
    assert_eq!(
        span.field("grpc.status"),
        Some("Ok"),
        "the status arrives in the trailers and is recorded at end of stream"
    );
    assert_eq!(span.field("access"), Some("authenticated"));
    assert!(span.has_field("latency_ms"));
    assert!(span.has_field("stream_ms"));
}

#[tokio::test]
async fn handler_sees_the_rpc_method_and_user_lands_on_the_rpc_span_despite_instrument() {
    let (capture, _guard) = capture_spans();
    let started = start(None).await;

    client(started.running.addr, &make_jwt(&[]))
        .get_end_point_one(IdList { id: vec![1] })
        .await
        .unwrap();

    assert_eq!(
        *started.seen_method.lock().unwrap(),
        Some(RpcMethod {
            service: SERVICE_NAME.to_string(),
            method: "getEndPointOne".to_string(),
        })
    );

    let rpc = rpc_span(&capture, "getEndPointOne");
    assert_eq!(rpc.field("user"), Some(TEST_USER));

    // `Span::current()` here is the handler's span, which lacks a `user`
    // field; `user` landing on the rpc span shows the gate records there.
    let handler_spans = capture.spans("get_end_point_one");
    assert_eq!(handler_spans.len(), 1, "the handler's #[instrument] span");
    assert!(!handler_spans[0].has_field("user"));
}

#[tokio::test]
async fn auth_rejection_is_traced_with_its_status_and_no_user() {
    let (capture, _guard) = capture_spans();
    let started = start(None).await;

    let err = client(started.running.addr, "")
        .get_end_point_one(IdList { id: vec![1] })
        .await
        .unwrap_err();
    assert_eq!(err.code(), Code::Unauthenticated);

    let span = rpc_span(&capture, "getEndPointOne");
    assert_eq!(
        span.field("grpc.status"),
        Some("Unauthenticated"),
        "the layer sits outside the gate, so it sees the rejection"
    );
    assert_eq!(span.field("access"), Some("authenticated"));
    assert!(!span.has_field("user"));
    assert!(
        capture.spans("get_end_point_one").is_empty(),
        "the handler must not have run"
    );
}

#[tokio::test]
async fn health_checks_are_traced_without_a_token() {
    let (capture, _guard) = capture_spans();
    let started = start(None).await;

    health_status(started.running.addr, "")
        .await
        .expect("health needs no token");

    let span = rpc_span(&capture, "Check");
    assert_eq!(span.field("rpc.service"), Some("grpc.health.v1.Health"));
    assert_eq!(span.field("grpc.status"), Some("Ok"));
    assert!(!span.has_field("user"));
    assert!(
        !span.has_field("access"),
        "health is not behind the gate, so it has no policy to record"
    );
}

#[tokio::test]
async fn timed_out_rpc_is_recorded_as_cancelled() {
    let (capture, _guard) = capture_spans();
    let started = start(Some(Duration::from_millis(100))).await;

    let err = client(started.running.addr, &make_jwt(&[]))
        .get_end_point_two(IdList { id: vec![1] })
        .await
        .unwrap_err();
    assert_eq!(err.code(), Code::Cancelled);

    // The timeout drops the response future, so the layer records
    // `Cancelled`; the gate already ran and recorded `user`.
    let span = rpc_span(&capture, "getEndPointTwo");
    assert_eq!(span.field("grpc.status"), Some("Cancelled"));
    assert_eq!(span.field("user"), Some(TEST_USER));
}
