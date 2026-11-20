//! Shared assertions for the isolated Unix signal test binaries.

use std::{process::Command, time::Duration};

use rust_grpc_lib::server::GrpcServer;
use rust_grpc_lib::testing::{
    ServingStatus, StreamExt, health_status, health_watch, test_validator,
};
use rust_grpc_lib::tonic::transport::Endpoint;
use tokio::net::TcpListener;
use tokio::time::{sleep, timeout};

const DEADLINE: Duration = Duration::from_secs(5);

pub async fn assert_graceful_shutdown(signal: &str) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);

    let server = tokio::spawn(
        GrpcServer::builder(test_validator("shutdown-signal"))
            .without_telemetry()
            .serve(addr),
    );
    let endpoint = Endpoint::from_shared(format!("http://{addr}")).unwrap();
    let mut watch = timeout(DEADLINE, async {
        loop {
            if let Ok(channel) = endpoint.connect().await
                && matches!(
                    health_status(channel.clone(), "").await,
                    Ok(ServingStatus::Serving)
                )
            {
                let mut watch = health_watch(channel, "").await.unwrap();
                assert_eq!(watch.next().await.unwrap().unwrap(), ServingStatus::Serving);
                break watch;
            }
            assert!(!server.is_finished(), "server exited before becoming ready");
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("server must become healthy before sending a signal");

    let status = Command::new("kill")
        .args([signal, &std::process::id().to_string()])
        .status()
        .expect("run POSIX kill");
    assert!(status.success(), "kill failed: {status}");

    let next = timeout(DEADLINE, watch.next())
        .await
        .expect("health must change when shutdown begins")
        .unwrap()
        .unwrap();
    assert_eq!(next, ServingStatus::NotServing);
    assert!(
        !server.is_finished(),
        "the open stream must delay the drain"
    );

    drop(watch);
    timeout(DEADLINE, server)
        .await
        .expect("server must finish after the stream is dropped")
        .unwrap()
        .expect("a graceful drain must return Ok(())");
}
