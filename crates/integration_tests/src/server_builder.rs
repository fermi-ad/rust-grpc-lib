//! `GrpcServer::builder` on a live server: health, shutdown, timeout and bind
//! errors. Spec: `rust_grpc_lib::server`; gate enforcement is in
//! `access_gate`, span coverage in `trace_layer`.

use std::time::Duration;

use tokio::net::TcpListener;
use tokio::time::{sleep, timeout};

use rust_grpc_lib::access_controlled_service;
use rust_grpc_lib::auth::{KeyValidator, RequestContext};
use rust_grpc_lib::server::{GrpcServer, GrpcServerBuilder, ServeError};
use rust_grpc_lib::testing::{ServingStatus, StreamExt, health_watch};
use rust_grpc_lib::tonic::{Code, Response, Status};

use crate::services::example::{
    IdList, ReplyOne, ReplyTwo,
    example_db_server::{ExampleDb, ExampleDbServer, SERVICE_NAME},
};
use crate::support::{client, connect, health_status, make_jwt, make_validator, serve};

/// `getEndPointTwo` sleeps 30 s, to trip the timeout.
#[derive(Clone)]
struct Stub;

#[access_controlled_service]
#[rust_grpc_lib::async_trait]
impl ExampleDb for Stub {
    #[any_authenticated]
    async fn get_end_point_one(
        &self,
        _ctx: RequestContext<IdList>,
    ) -> Result<Response<ReplyOne>, Status> {
        Ok(Response::new(ReplyOne {
            section_one: vec![],
        }))
    }

    #[any_authenticated]
    async fn get_end_point_two(
        &self,
        _ctx: RequestContext<IdList>,
    ) -> Result<Response<ReplyTwo>, Status> {
        sleep(Duration::from_secs(30)).await;
        Err(Status::unimplemented("unreachable in the tests"))
    }
}

fn builder() -> GrpcServerBuilder<KeyValidator> {
    GrpcServer::builder(make_validator()).add_service(ExampleDbServer::new, Stub)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn health_needs_no_token_and_reports_every_service() {
    let running = serve(builder()).await;

    assert_eq!(
        health_status(running.addr, "").await.unwrap(),
        ServingStatus::Serving
    );
    assert_eq!(
        health_status(running.addr, SERVICE_NAME).await.unwrap(),
        ServingStatus::Serving
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn health_turns_not_serving_when_shutdown_begins() {
    let running = serve(builder()).await;

    // A `Watch` stream still gets updates while the server drains.
    let mut watch = health_watch(connect(running.addr).await, SERVICE_NAME)
        .await
        .unwrap();
    let first = watch.next().await.unwrap().unwrap();
    assert_eq!(first, ServingStatus::Serving);

    running.shutdown.send(()).unwrap();

    let next = timeout(Duration::from_secs(5), watch.next())
        .await
        .expect("a status update must arrive when shutdown begins")
        .unwrap()
        .unwrap();
    assert_eq!(next, ServingStatus::NotServing);

    // Graceful shutdown waits for the open stream.
    drop(watch);
    let result = timeout(Duration::from_secs(5), running.task)
        .await
        .expect("the server must stop after shutdown")
        .unwrap();
    assert!(result.is_ok(), "{result:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn timeout_applies_to_application_rpcs() {
    let running = serve(builder().timeout(Duration::from_millis(100))).await;

    let err = client(running.addr, &make_jwt(&[]))
        .get_end_point_two(IdList { id: vec![1] })
        .await
        .unwrap_err();
    assert_eq!(err.code(), Code::Cancelled);
}

#[tokio::test]
async fn serving_on_an_occupied_port_is_a_bind_error() {
    let occupied = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = occupied.local_addr().unwrap();

    // `Ok` is `Infallible`, so this pattern is irrefutable.
    let Err(err) = builder().serve(addr).await;

    assert!(matches!(err, ServeError::Bind(_)), "{err:?}");
}
