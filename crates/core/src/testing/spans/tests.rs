//! Tests for the span capture in [`super`].

use super::*;

use tracing::field;
use tracing::info_span;

#[test]
fn capture_spans_records_initial_and_later_values() {
    let (capture, _guard) = capture_spans();

    let span = info_span!("work", kind = "initial", later = field::Empty);
    assert_eq!(capture.field("work", "kind").as_deref(), Some("initial"));
    assert_eq!(capture.field("work", "later"), None);

    span.record("later", "filled in");
    let records = capture.spans("work");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].name(), "work");
    assert_eq!(records[0].field("later"), Some("filled in"));
    assert!(records[0].has_field("kind"));
    assert!(!records[0].has_field("missing"));
}

#[test]
fn capture_spans_keeps_spans_after_they_close() {
    let (capture, _guard) = capture_spans();

    drop(info_span!("short", n = 1));
    drop(info_span!("short", n = 2));

    let records = capture.spans("short");
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].field("n"), Some("1"));
    assert_eq!(records[1].field("n"), Some("2"));
}
