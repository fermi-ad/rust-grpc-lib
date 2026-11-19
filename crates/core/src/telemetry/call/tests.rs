//! Tests for the shared completion guard.

use super::*;

use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone, Default)]
struct Collector(Rc<RefCell<Vec<CallSummary>>>);

impl Sink for Collector {
    fn started(&self, _method: Option<&'static str>) {}

    fn finished(&self, summary: CallSummary) {
        self.0.borrow_mut().push(summary);
    }
}

#[test]
fn transfer_preserves_start_time_and_only_one_terminal_status() {
    let sink = Collector::default();
    let elapsed = Duration::from_millis(25);
    let mut future = CompletionGuard::new(
        sink.clone(),
        Some("pkg.Service/Method"),
        Instant::now() - elapsed,
    );
    let mut body = future.transfer();
    drop(future);
    assert!(sink.0.borrow().is_empty());

    body.finish(Code::Ok);
    body.finish(Code::Unknown);
    drop(body);

    let seen = sink.0.borrow();
    let [summary] = seen.as_slice() else {
        panic!("expected one completion, got {seen:?}");
    };
    assert_eq!(summary.method, Some("pkg.Service/Method"));
    assert_eq!(summary.status, Code::Ok);
    assert!(summary.duration >= elapsed);
}
