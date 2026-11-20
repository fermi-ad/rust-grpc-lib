use std::{
    sync::{Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

use opentelemetry_sdk::{
    error::{OTelSdkError, OTelSdkResult},
    metrics::{InstrumentKind, Pipeline, Temporality, data::ResourceMetrics, reader::MetricReader},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpSocket},
};

use super::super::{Telemetry, TelemetryConfig, tests::capture_logs};
use super::SHUTDOWN_TIMEOUT;

#[test]
fn invalid_endpoint_degrades_to_noop_and_does_not_log_the_url() {
    let (logs, _guard) = capture_logs();
    let telemetry = Telemetry::start(
        TelemetryConfig::default().endpoint("ftp://localhost/metrics?token=secret-value"),
    );
    assert!(!telemetry.is_enabled());
    assert!(logs.text().contains("invalid OTLP/HTTP endpoint"));
    assert!(!logs.text().contains("secret-value"));
}

#[test]
fn starts_outside_tokio() {
    let telemetry =
        Telemetry::start(TelemetryConfig::default().endpoint("http://127.0.0.1:9/v1/metrics"));
    assert!(telemetry.is_enabled());
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap()
        .block_on(telemetry.shutdown());
}

#[tokio::test]
async fn shutdown_posts_metrics_over_otlp_http() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/custom/metrics", listener.local_addr().unwrap());
    let received = tokio::spawn(async move {
        let mut first = None;
        // The SDK exports both on force_flush and during reader shutdown.
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut headers = Vec::new();
            while !headers.ends_with(b"\r\n\r\n") {
                headers.push(stream.read_u8().await.unwrap());
                assert!(headers.len() <= 16 * 1024);
            }
            let headers = String::from_utf8(headers).unwrap().to_ascii_lowercase();
            let length = headers
                .lines()
                .find_map(|line| line.strip_prefix("content-length:")?.trim().parse().ok())
                .unwrap();
            let mut body = vec![0; length];
            stream.read_exact(&mut body).await.unwrap();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
            first.get_or_insert((headers, body));
        }
        first.unwrap()
    });

    let (logs, _guard) = capture_logs();
    let telemetry = Telemetry::start(
        TelemetryConfig::default()
            .endpoint(endpoint)
            .export_interval(Duration::from_secs(600)),
    );
    assert!(telemetry.is_enabled());
    telemetry
        .meter()
        .u64_counter("test.loopback.calls")
        .build()
        .add(1, &[]);
    telemetry.shutdown().await;
    let (headers, body) = tokio::time::timeout(Duration::from_secs(1), received)
        .await
        .unwrap()
        .unwrap();
    assert!(headers.starts_with("post /custom/metrics http/1.1\r\n"));
    assert!(headers.contains("content-type: application/x-protobuf\r\n"));
    assert!(
        body.windows(b"test.loopback.calls".len())
            .any(|bytes| bytes == b"test.loopback.calls")
    );
    assert!(!logs.text().contains("telemetry flush or shutdown failed"));
}

#[tokio::test]
async fn unreachable_endpoint_does_not_block_start_or_shutdown() {
    // Reserve an ephemeral port without listening, so another test cannot take it.
    let socket = TcpSocket::new_v4().unwrap();
    socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let endpoint = format!("http://{}/v1/metrics", socket.local_addr().unwrap());
    let begin = Instant::now();
    let telemetry = Telemetry::start(TelemetryConfig::default().endpoint(endpoint));
    assert!(telemetry.is_enabled());
    assert!(begin.elapsed() < SHUTDOWN_TIMEOUT);
    telemetry
        .meter()
        .u64_counter("test.calls")
        .build()
        .add(1, &[]);
    let begin = Instant::now();
    telemetry.shutdown().await;
    assert!(begin.elapsed() < SHUTDOWN_TIMEOUT + Duration::from_secs(1));
}

#[tokio::test]
async fn failed_flush_warns_and_still_stops_the_provider() {
    let (logs, _guard) = capture_logs();
    let (events_tx, events_rx) = mpsc::channel();
    let flushed = events_tx.clone();
    let telemetry = Telemetry::from_reader(
        LifecycleReader {
            on_flush: Box::new(move || {
                assert!(tokio::runtime::Handle::try_current().is_err());
                flushed.send("flush").unwrap();
                Err(OTelSdkError::InternalFailure("test failure".into()))
            }),
            on_shutdown: Box::new(move || {
                events_tx.send("shutdown").unwrap();
                Ok(())
            }),
        },
        TelemetryConfig::default(),
    );
    let clone = telemetry.clone();
    telemetry.shutdown().await;
    clone.shutdown().await;
    assert_eq!(
        events_rx.try_iter().collect::<Vec<_>>(),
        ["flush", "shutdown"]
    );
    assert!(logs.text().contains("telemetry flush or shutdown failed"));
}

#[tokio::test(start_paused = true)]
async fn timeout_returns_while_flush_is_blocked_and_shutdown_continues() {
    let (logs, _guard) = capture_logs();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let release_rx = Mutex::new(release_rx);
    let (stopped_tx, stopped_rx) = mpsc::channel();
    let telemetry = Telemetry::from_reader(
        LifecycleReader {
            on_flush: Box::new(move || {
                entered_tx.send(()).unwrap();
                release_rx
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(10))
                    .unwrap();
                Ok(())
            }),
            on_shutdown: Box::new(move || {
                stopped_tx.send(thread::current().id()).unwrap();
                Ok(())
            }),
        },
        TelemetryConfig::default(),
    );
    let clone = telemetry.clone();
    let begin = tokio::time::Instant::now();
    let mut shutdown = Box::pin(telemetry.shutdown());
    assert!(futures_util::poll!(&mut shutdown).is_pending());
    entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    tokio::time::advance(SHUTDOWN_TIMEOUT - Duration::from_millis(1)).await;
    assert!(futures_util::poll!(&mut shutdown).is_pending());
    clone.shutdown().await;
    shutdown.await;
    assert!(
        (SHUTDOWN_TIMEOUT..=SHUTDOWN_TIMEOUT + Duration::from_millis(1)).contains(&begin.elapsed())
    );
    assert!(
        logs.text()
            .contains("telemetry shutdown timed out after five seconds")
    );
    assert!(stopped_rx.try_recv().is_err());
    release_tx.send(()).unwrap();
    assert_ne!(
        stopped_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        thread::current().id()
    );
}

#[tokio::test]
async fn dropping_last_clone_stops_provider_off_runtime() {
    let (stopped_tx, stopped_rx) = mpsc::channel();
    let telemetry = Telemetry::from_reader(
        LifecycleReader {
            on_flush: Box::new(|| panic!("drop must not force a separate flush")),
            on_shutdown: Box::new(move || {
                stopped_tx.send(thread::current().id()).unwrap();
                Ok(())
            }),
        },
        TelemetryConfig::default(),
    );
    let clone = telemetry.clone();
    drop(telemetry);
    assert!(clone.is_enabled());
    drop(clone);
    assert_ne!(
        stopped_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        thread::current().id()
    );
}

struct LifecycleReader {
    on_flush: Box<dyn Fn() -> OTelSdkResult + Send + Sync>,
    on_shutdown: Box<dyn Fn() -> OTelSdkResult + Send + Sync>,
}

impl std::fmt::Debug for LifecycleReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LifecycleReader")
    }
}

impl MetricReader for LifecycleReader {
    fn register_pipeline(&self, _: std::sync::Weak<Pipeline>) {}

    fn collect(&self, _: &mut ResourceMetrics) -> OTelSdkResult {
        Ok(())
    }

    fn force_flush(&self) -> OTelSdkResult {
        (self.on_flush)()
    }

    fn shutdown_with_timeout(&self, _: Duration) -> OTelSdkResult {
        (self.on_shutdown)()
    }

    fn temporality(&self, _: InstrumentKind) -> Temporality {
        Temporality::Cumulative
    }
}
