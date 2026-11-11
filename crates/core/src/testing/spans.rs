//! Span capture for asserting on `tracing` span fields (see [`capture_spans`]).

use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::{Arc, Mutex, PoisonError};

use tracing::Subscriber;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::subscriber::DefaultGuard;
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

#[cfg(test)]
mod tests;

/// Handle to the spans recorded since [`capture_spans`]. Clones share the
/// recording; spans stay available after they close.
#[derive(Clone, Default)]
pub struct SpanCapture(Arc<Mutex<State>>);

impl SpanCapture {
    /// The spans named `name`, oldest first.
    // A poisoned lock is recovered so one panicking test does not hide spans
    // another wants to assert on.
    #[must_use = "this returns a snapshot of the captured spans; it has no effect unless you inspect it"]
    pub fn spans(&self, name: &str) -> Vec<SpanRecord> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .spans
            .iter()
            .filter(|span| span.name == name)
            .cloned()
            .collect()
    }

    /// The value of `field` on the first span named `span_name`, or `None` if
    /// there is no such span or the field was never recorded.
    #[must_use = "this only looks up a recorded value; it has no effect unless you inspect it"]
    pub fn field(&self, span_name: &str, field: &str) -> Option<String> {
        self.spans(span_name)
            .first()
            .and_then(|span| span.field(field).map(str::to_owned))
    }
}

impl<S: Subscriber> Layer<S> for SpanCapture {
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, _ctx: Context<'_, S>) {
        let mut fields = HashMap::new();
        attrs.record(&mut FieldVisitor(&mut fields));
        let mut state = self.0.lock().unwrap();
        let index = state.spans.len();
        state.spans.push(SpanRecord {
            name: attrs.metadata().name().to_string(),
            fields,
        });
        state.open.insert(id.into_u64(), index);
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, _ctx: Context<'_, S>) {
        let mut state = self.0.lock().unwrap();
        if let Some(&index) = state.open.get(&id.into_u64()) {
            values.record(&mut FieldVisitor(&mut state.spans[index].fields));
        }
    }

    fn on_close(&self, id: Id, _ctx: Context<'_, S>) {
        self.0.lock().unwrap().open.remove(&id.into_u64());
    }
}

/// One captured span and the field values recorded on it so far. Strings are
/// stored verbatim, everything else in `Debug` form (so a `tonic::Code`
/// recorded with `field::debug` reads `"Ok"`).
#[derive(Clone, Debug)]
pub struct SpanRecord {
    name: String,
    fields: HashMap<String, String>,
}

impl SpanRecord {
    /// The span's name, such as `rpc`.
    #[must_use = "this only reads the span name; it has no effect unless you inspect it"]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The value of `field`, or `None` if never recorded (including a field
    /// declared `Empty` and never filled in).
    pub fn field(&self, field: &str) -> Option<&str> {
        self.fields.get(field).map(String::as_str)
    }

    /// Whether `field` has a recorded value.
    #[must_use = "this only reports whether the field was recorded; assert on the result"]
    pub fn has_field(&self, field: &str) -> bool {
        self.fields.contains_key(field)
    }
}

/// Installs a subscriber on the current thread that records every span and
/// the values recorded on it (spans only, not events).
///
/// Capture lasts until the returned guard drops, so keep it alive for the
/// whole test. The subscriber is thread-local: use the default current-thread
/// `#[tokio::test]` runtime, since a multi-threaded one runs the code on
/// threads that never see it, and those spans are lost.
///
/// ```rust
/// use rust_grpc_lib::testing::capture_spans;
///
/// let (spans, _guard) = capture_spans();
/// tracing::info_span!("rpc", grpc.status = "Ok").in_scope(|| {}); // code under test
/// assert_eq!(spans.field("rpc", "grpc.status").as_deref(), Some("Ok"));
/// ```
#[must_use = "dropping the guard immediately stops span capture, and the capture is useless unless you read it"]
pub fn capture_spans() -> (SpanCapture, DefaultGuard) {
    let capture = SpanCapture::default();
    let subscriber = tracing_subscriber::registry().with(capture.clone());
    (capture, tracing::subscriber::set_default(subscriber))
}

// `spans` is append-only so records outlive their spans. `open` maps live span
// ids to indices in `spans`; the registry reuses ids, so entries go on close.
#[derive(Default)]
struct State {
    spans: Vec<SpanRecord>,
    open: HashMap<u64, usize>,
}

// Collects field values as strings: `str` verbatim, everything else via `Debug`.
struct FieldVisitor<'a>(&'a mut HashMap<String, String>);

impl Visit for FieldVisitor<'_> {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
        self.0
            .insert(field.name().to_string(), format!("{value:?}"));
    }
}
