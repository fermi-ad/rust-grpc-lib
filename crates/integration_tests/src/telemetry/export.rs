//! Local OTLP/HTTP test listener. Decode only the protobuf fields asserted
//! here, using the same prost re-export as the generated gRPC fixture.

use std::process::Command;

use rust_grpc_lib::prost::Message;
use rust_grpc_lib::telemetry::{DEFAULT_DURATION_BUCKET_BOUNDS_SECONDS, Telemetry};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{self, UnboundedReceiver};
use tokio::task::JoinHandle;

use super::*;
use crate::support::capture_logs;

const PROBE: &str = "runtime.scheduling_delay";
const COUNTER: &str = "test.consumer.calls";
const HISTOGRAM: &str = "test.consumer.value";

#[derive(Clone, PartialEq, Message)]
#[prost(prost_path = "::rust_grpc_lib::prost")]
struct Export {
    #[prost(message, repeated, tag = "1")]
    resources: Vec<ResourceMetrics>,
}

#[derive(Clone, PartialEq, Message)]
#[prost(prost_path = "::rust_grpc_lib::prost")]
struct ResourceMetrics {
    #[prost(message, repeated, tag = "2")]
    scopes: Vec<ScopeMetrics>,
}

#[derive(Clone, PartialEq, Message)]
#[prost(prost_path = "::rust_grpc_lib::prost")]
struct ScopeMetrics {
    #[prost(message, repeated, tag = "2")]
    metrics: Vec<Metric>,
}

#[derive(Clone, PartialEq, Message)]
#[prost(prost_path = "::rust_grpc_lib::prost")]
struct Metric {
    #[prost(string, tag = "1")]
    name: String,
    #[prost(string, tag = "3")]
    unit: String,
    #[prost(message, optional, tag = "7")]
    sum: Option<Sum>,
    #[prost(message, optional, tag = "9")]
    histogram: Option<Histogram>,
}

#[derive(Clone, PartialEq, Message)]
#[prost(prost_path = "::rust_grpc_lib::prost")]
struct Sum {
    #[prost(message, repeated, tag = "1")]
    points: Vec<Number>,
}

#[derive(Clone, PartialEq, Message)]
#[prost(prost_path = "::rust_grpc_lib::prost")]
struct Number {
    #[prost(sfixed64, tag = "6")]
    value: i64,
}

#[derive(Clone, PartialEq, Message)]
#[prost(prost_path = "::rust_grpc_lib::prost")]
struct Histogram {
    #[prost(message, repeated, tag = "1")]
    points: Vec<Point>,
}

#[derive(Clone, PartialEq, Message)]
#[prost(prost_path = "::rust_grpc_lib::prost")]
struct Point {
    #[prost(fixed64, tag = "4")]
    count: u64,
    #[prost(double, repeated, tag = "7")]
    bounds: Vec<f64>,
    #[prost(double, optional, tag = "12")]
    max: Option<f64>,
}

impl Export {
    fn metric(&self, name: &str) -> Option<&Metric> {
        self.resources
            .iter()
            .flat_map(|resource| &resource.scopes)
            .flat_map(|scope| &scope.metrics)
            .find(|metric| metric.name == name)
    }

    fn point(&self, name: &str) -> Option<&Point> {
        let histogram = self.metric(name)?.histogram.as_ref().unwrap();
        let [point] = histogram.points.as_slice() else {
            panic!("expected one point for {name}")
        };
        Some(point)
    }

    fn counter(&self, name: &str) -> Option<i64> {
        let sum = self.metric(name)?.sum.as_ref().unwrap();
        let [point] = sum.points.as_slice() else {
            panic!("expected one point for {name}")
        };
        Some(point.value)
    }
}

struct Collector {
    endpoint: String,
    exports: UnboundedReceiver<Export>,
    task: JoinHandle<()>,
}

impl Collector {
    async fn bind() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/test/metrics", listener.local_addr().unwrap());
        let (tx, exports) = mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let export = receive(stream).await;
                if tx.send(export).is_err() {
                    break;
                }
            }
        });
        Self {
            endpoint,
            exports,
            task,
        }
    }

    fn config(&self) -> TelemetryConfig {
        TelemetryConfig::default()
            .endpoint(self.endpoint.clone())
            .export_interval(Duration::from_secs(600))
    }

    async fn until(&mut self, predicate: impl Fn(&Export) -> bool) -> Export {
        timeout(DEADLINE, async {
            loop {
                let export = self
                    .exports
                    .recv()
                    .await
                    .expect("loopback collector stopped");
                if predicate(&export) {
                    return export;
                }
            }
        })
        .await
        .expect("no matching OTLP/HTTP export")
    }

    fn assert_no_export(&mut self) {
        assert!(
            !self.task.is_finished(),
            "loopback collector stopped unexpectedly"
        );
        assert!(
            self.exports.try_recv().is_err(),
            "unexpected OTLP/HTTP export"
        );
    }
}

impl Drop for Collector {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn receive(stream: TcpStream) -> Export {
    let mut stream = BufReader::new(stream);
    let mut line = String::new();
    stream.read_line(&mut line).await.unwrap();
    assert_eq!(line, "POST /test/metrics HTTP/1.1\r\n");
    let mut length = None;
    let mut protobuf = false;
    loop {
        line.clear();
        assert_ne!(stream.read_line(&mut line).await.unwrap(), 0);
        if line == "\r\n" {
            break;
        }
        let (name, value) = line.split_once(':').unwrap();
        if name.eq_ignore_ascii_case("content-length") {
            length = Some(value.trim().parse::<usize>().unwrap());
        }
        if name.eq_ignore_ascii_case("content-type") {
            protobuf = value.trim() == "application/x-protobuf";
        }
    }
    assert!(protobuf);
    let length = length.expect("OTLP request must have Content-Length");
    assert!((1..=64 * 1024).contains(&length));
    let mut body = vec![0; length];
    stream.read_exact(&mut body).await.unwrap();
    let export = Export::decode(body.as_slice()).unwrap();
    stream
        .get_mut()
        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    export
}

// Exporter and proxy settings belong to the process. The child avoids unsafe
// environment mutation and won't inherit ambient collectors, proxies or headers.
#[tokio::test]
async fn loopback_export_and_probe() {
    const CHILD: &str = "C14_TELEMETRY_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = tokio::task::spawn_blocking(|| {
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "telemetry::export::loopback_export_and_probe",
                    "--nocapture",
                ])
                .env_clear()
                .env(CHILD, "1")
                .env("GRPC_RUNTIME_PROBE", "false")
                .output()
                .unwrap()
        })
        .await
        .unwrap();
        assert!(
            output.status.success(),
            "isolated telemetry test failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        return;
    }

    no_endpoint_and_opt_outs().await;
    export_at_shutdown(false).await;
    export_at_shutdown(true).await;
    probe_samples_only_while_serving().await;
    probe_opt_outs().await;
}

async fn no_endpoint_and_opt_outs() {
    // With no endpoint the default source is a no-op (including the probe),
    // but it and the SDK-disabled source still run the call observer.
    assert!(!Telemetry::start(TelemetryConfig::from_env()).is_enabled());
    let mut collector = Collector::bind().await;
    let in_memory = InMemoryMetrics::new();
    let shared = in_memory.telemetry();
    for (builder, expected_observations) in [
        (builder(), 1),
        (
            builder().telemetry_config(
                collector
                    .config()
                    .sdk_disabled(true)
                    .runtime_scheduling_probe(true),
            ),
            1,
        ),
        (
            builder()
                .telemetry_config(collector.config())
                .without_telemetry(),
            0,
        ),
        (
            builder()
                .with_caller_owned_telemetry(&shared)
                .without_telemetry(),
            0,
        ),
    ] {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let builder = builder.on_call_complete(move |call| tx.send(*call).unwrap());
        builder.meter().u64_counter(COUNTER).build().add(1, &[]);
        let running = start(builder).await;
        call_one(&running).await;
        finish(running).await;
        let calls: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert_eq!(calls.len(), expected_observations);
        for call in calls {
            assert_eq!((call.method, call.status), (Some(ONE), Code::Ok));
        }
    }
    collector.assert_no_export();
    for metric in [CALL, PROBE] {
        assert!(in_memory.histograms(metric).is_empty());
    }
    assert!(in_memory.counters(COUNTER).is_empty());
    shared.shutdown().await;
}

async fn call_one(running: &Running) {
    timeout(
        DEADLINE,
        client(running.addr, &make_jwt(&["operator"])).get_end_point_one(IdList::default()),
    )
    .await
    .unwrap()
    .unwrap();
}

async fn export_at_shutdown(invalid: bool) {
    let (logs, _guard) = capture_logs();
    let mut collector = Collector::bind().await;
    let configured = [0.01, 0.05, 0.25];
    let (override_bounds, expected_bounds): (&[f64], &[f64]) = if invalid {
        (&[0.5, 0.1], &DEFAULT_DURATION_BUCKET_BOUNDS_SECONDS)
    } else {
        (&configured, &configured)
    };
    let config = collector
        .config()
        .runtime_scheduling_probe(false)
        .duration_bucket_bounds_seconds(override_bounds);
    let builder = builder().telemetry_config(config);
    builder.meter().u64_counter(COUNTER).build().add(7, &[]);
    builder
        .meter()
        .f64_histogram(HISTOGRAM)
        .build()
        .record(0.04, &[]);
    let running = start(builder).await;
    call_one(&running).await;
    collector.assert_no_export(); // No periodic export at the deliberately long interval.
    finish(running).await;
    let export = collector.until(|_| true).await;
    assert_eq!(export.counter(COUNTER), Some(7));
    assert_eq!(export.point(HISTOGRAM).unwrap().count, 1);
    let call = export.point(CALL).unwrap();
    assert_eq!(call.count, 1);
    assert_eq!(call.bounds, expected_bounds);
    assert_eq!(export.metric(CALL).unwrap().unit, "s");
    assert!(export.metric(PROBE).is_none());
    let logs = logs.text();
    assert_eq!(
        logs.contains("invalid duration buckets; using defaults"),
        invalid
    );
    assert!(!logs.contains("telemetry flush or shutdown failed"));
    assert!(!logs.contains("telemetry shutdown timed out"));
}

async fn probe_samples_only_while_serving() {
    let mut collector = Collector::bind().await;
    let telemetry = Telemetry::start(
        collector
            .config()
            .runtime_scheduling_probe(true)
            .export_interval(Duration::from_millis(50)),
    );
    let marker = telemetry.meter().u64_counter(COUNTER).build();
    marker.add(1, &[]);
    assert!(
        collector
            .until(|export| export.counter(COUNTER) == Some(1))
            .await
            .metric(PROBE)
            .is_none()
    );
    let running = start(builder().with_caller_owned_telemetry(&telemetry)).await;
    call_one(&running).await;
    let before = collector
        .until(|export| export.point(PROBE).is_some())
        .await;
    assert_eq!(before.metric(PROBE).unwrap().unit, "s");
    let count = before.point(PROBE).unwrap().count;
    // Current-thread runtime: block a due probe tick while the server is live.
    std::thread::sleep(Duration::from_millis(65));
    let delayed = collector
        .until(|export| {
            export.point(PROBE).is_some_and(|point| {
                point.count > count && point.max.is_some_and(|max| max >= 0.04)
            })
        })
        .await;
    assert!(delayed.point(PROBE).unwrap().count > count);
    finish(running).await;
    assert!(telemetry.is_enabled());
    marker.add(1, &[]);
    let stopped = collector
        .until(|export| export.counter(COUNTER) == Some(2))
        .await;
    let after = collector
        .until(|export| export.counter(COUNTER) == Some(2))
        .await;
    assert_eq!(after.point(PROBE), stopped.point(PROBE));
    telemetry.shutdown().await;
}

async fn probe_opt_outs() {
    let mut collector = Collector::bind().await;
    // Config and inherited env are independent opt-outs. Give each an
    // export interval while still serving: checking only after immediate
    // shutdown could pass even if the probe had incorrectly started.
    for config in [
        collector.config().runtime_scheduling_probe(false),
        TelemetryConfig::from_env().endpoint(collector.endpoint.clone()),
    ] {
        let builder = builder().telemetry_config(config.export_interval(Duration::from_millis(50)));
        builder.meter().u64_counter(COUNTER).build().add(1, &[]);
        let running = start(builder).await;
        call_one(&running).await;
        let export = collector
            .until(|export| export.metric(CALL).is_some())
            .await;
        assert!(export.metric(PROBE).is_none());
        finish(running).await;
        while let Ok(export) = collector.exports.try_recv() {
            assert!(export.metric(PROBE).is_none());
        }
    }
}
