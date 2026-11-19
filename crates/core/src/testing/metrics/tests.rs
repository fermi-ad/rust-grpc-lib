use super::*;
use crate::telemetry::DEFAULT_DURATION_BUCKETS;

#[test]
fn histograms_snapshot_cumulative_points_and_configured_bounds() {
    let metrics = InMemory::new();
    let telemetry = metrics.telemetry();
    let duration = telemetry.duration_histogram_with("test.duration", "latency", &[0.25, 0.5]);
    let default = telemetry.duration_histogram("test.default", "latency");
    assert!(metrics.histograms("test.duration").is_empty());
    let attrs = [
        KeyValue::new("worker", "first"),
        KeyValue::new("attempt", 2_i64),
        KeyValue::new("cached", false),
    ];
    duration.record(0.125, &attrs);
    assert_eq!(metrics.histograms("test.duration")[0].count, 1);
    duration.record(0.5, &attrs);
    duration.record(1.0, &attrs);
    default.record(0.125, &[]);

    let points = metrics.histograms("test.duration");
    assert_eq!(points, metrics.histograms("test.duration"));
    assert_eq!(
        points,
        [HistogramSnapshot {
            unit: "s".into(),
            attributes: BTreeMap::from([
                ("attempt".into(), "2".into()),
                ("cached".into(), "false".into()),
                ("worker".into(), "first".into()),
            ]),
            count: 3,
            sum: 1.625,
            min: Some(0.125),
            max: Some(1.0),
            bounds: vec![0.25, 0.5],
            bucket_counts: vec![1, 1, 1],
        }]
    );
    assert_eq!(points[0].bucket_counts.iter().sum::<u64>(), points[0].count);
    assert_eq!(
        metrics.histograms("test.default")[0].bounds,
        DEFAULT_DURATION_BUCKETS
    );
    assert!(metrics.counters("test.duration").is_empty());
}

#[test]
fn counters_include_signed_up_down_points_and_sort_by_attributes() {
    let metrics = InMemory::new();
    let meter = metrics.telemetry().meter();
    let counter = meter.u64_counter("test.completed").build();
    counter.add(2, &[KeyValue::new("queue", "b")]);
    counter.add(3, &[KeyValue::new("queue", "a")]);
    counter.add(5, &[KeyValue::new("queue", "a")]);
    let active = meter.i64_up_down_counter("test.active").build();
    active.add(1, &[]);
    assert_eq!(metrics.counters("test.active")[0].value, 1.0);
    active.add(-3, &[]);
    assert_eq!(metrics.counters("test.active")[0].value, -2.0);
    assert_eq!(
        metrics.counters("test.completed"),
        [
            CounterSnapshot {
                attributes: BTreeMap::from([("queue".into(), "a".into())]),
                value: 8.0
            },
            CounterSnapshot {
                attributes: BTreeMap::from([("queue".into(), "b".into())]),
                value: 2.0
            },
        ]
    );
    assert!(metrics.histograms("test.completed").is_empty());
}

#[tokio::test]
async fn instances_are_isolated_and_keep_final_metrics_after_shutdown() {
    let first = InMemory::new();
    let second = InMemory::new();
    let telemetry = first.telemetry();
    let counter = telemetry.meter().u64_counter("test.completed").build();
    let other = second
        .telemetry()
        .meter()
        .u64_counter("test.completed")
        .build();
    counter.add(1, &[]);
    assert!(second.counters("test.completed").is_empty());
    other.add(10, &[]);
    assert_eq!(first.counters("test.completed")[0].value, 1.0);
    counter.add(2, &[]);
    telemetry.shutdown().await;
    assert!(!first.telemetry().is_enabled());
    assert_eq!(first.counters("test.completed")[0].value, 3.0);
    other.add(1, &[]);
    assert_eq!(second.counters("test.completed")[0].value, 11.0);
}

#[tokio::test(start_paused = true)]
async fn calls_waits_for_the_requested_method_and_status() {
    let metrics = InMemory::new();
    let duration = metrics
        .telemetry()
        .duration_histogram(CALL_DURATION, "call duration");
    let record = move |method: &'static str, code: Code| {
        duration.record(
            0.125,
            &[
                KeyValue::new("rpc.method", method),
                KeyValue::new("rpc.response.status_code", code as i64),
            ],
        );
    };
    let method = "test.Service/Method";
    record(method, Code::Ok);
    record("test.Service/Other", Code::Cancelled);
    let task = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(25)).await;
        record(method, Code::Cancelled);
        record(method, Code::Cancelled);
    });
    assert_eq!(metrics.calls(method, Code::Cancelled).await, 2);
    task.await.unwrap();
}

#[tokio::test(start_paused = true)]
#[should_panic(
    expected = "waiting for calls for \"test.Service/Missing\" with Cancelled; recorded series: [\"rpc.server.call.duration:"
)]
async fn calls_timeout_lists_observed_series() {
    let metrics = InMemory::new();
    metrics
        .telemetry()
        .duration_histogram(CALL_DURATION, "call duration")
        .record(
            0.125,
            &[
                KeyValue::new("rpc.method", "test.Service/Observed"),
                KeyValue::new("rpc.response.status_code", Code::Ok as i64),
            ],
        );
    metrics.calls("test.Service/Missing", Code::Cancelled).await;
}

#[tokio::test(start_paused = true)]
async fn wait_until_polls_fresh_data_and_checks_immediately() {
    let metrics = InMemory::new();
    metrics
        .wait_until(|m| m.counters("test.late").is_empty(), Duration::ZERO)
        .await;
    let counter = metrics.telemetry().meter().u64_counter("test.late").build();
    let task = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(25)).await;
        counter.add(4, &[]);
    });
    metrics
        .wait_until(
            |m| {
                m.counters("test.late")
                    .iter()
                    .any(|point| point.value == 4.0)
            },
            Duration::from_millis(100),
        )
        .await;
    task.await.unwrap();
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "timed out after 17ms waiting for metric predicate; recorded series: []")]
async fn wait_until_uses_the_supplied_timeout() {
    InMemory::new()
        .wait_until(|_| false, Duration::from_millis(17))
        .await;
}
