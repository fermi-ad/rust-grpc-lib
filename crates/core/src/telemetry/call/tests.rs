//! Tests for the shared completion guard.

use super::*;

use std::sync::Mutex;

use tokio::sync::watch;

use crate::telemetry::{Telemetry, TelemetryConfig};

#[test]
fn transfer_preserves_start_time_and_only_one_terminal_status() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&seen);
    let recorder = Arc::new(CallRecorder::new(
        &Telemetry::start(TelemetryConfig::default()),
        Some(Arc::new(move |summary| {
            observed.lock().unwrap().push(*summary)
        })),
        watch::channel(()).1,
    ));
    let elapsed = Duration::from_millis(25);
    let mut future = CompletionGuard::new(
        recorder,
        Some("pkg.Service/Method"),
        Instant::now() - elapsed,
    );
    let mut body = future.take_completion();
    drop(future);
    assert!(seen.lock().unwrap().is_empty());

    body.finish(Code::Ok);
    body.finish(Code::Unknown);
    drop(body);

    let seen = seen.lock().unwrap();
    let [summary] = seen.as_slice() else {
        panic!("expected one completion, got {seen:?}");
    };
    assert_eq!(summary.method, Some("pkg.Service/Method"));
    assert_eq!(summary.status, Code::Ok);
    assert!(summary.duration >= elapsed);
}
