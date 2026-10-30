//! Capture `tracing` spans in a test.
//!
//! [`capture_spans`] installs a thread-local subscriber that remembers every
//! span and the values recorded onto it, so a test can assert on span fields,
//! for example that `GrpcTraceLayer` recorded `grpc.status` or that
//! `RequestContext` recorded `user`.
//!
//! The subscriber is thread-local, so run such tests on the default
//! current-thread `#[tokio::test]` runtime. A multi-threaded runtime moves the
//! work to threads that never see it.
//!
//! ```rust,ignore
//! let (spans, _guard) = capture_spans();
//! // ... run the code under test ...
//! assert_eq!(spans.field("rpc", "grpc.status").as_deref(), Some("Ok"));
//! ```

use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::{Arc, Mutex};

use tracing::Subscriber;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::subscriber::DefaultGuard;
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

/// A `tracing` layer that remembers the fields of every span.
///
/// Create one with [`capture_spans`].
#[derive(Clone, Default)]
pub struct SpanCapture(Arc<Mutex<State>>);

/// One span and every field value recorded onto it so far.
///
/// String values are stored as they are, everything else in its `Debug`
/// form (so a `tonic::Code` recorded with `field::debug` reads `"Ok"`).
#[derive(Clone, Debug)]
pub struct SpanRecord {
    name: String,
    fields: HashMap<String, String>,
}

impl SpanCapture {
    /// The spans with the given name, oldest first.
    pub fn spans(&self, name: &str) -> Vec<SpanRecord> {
        self.0
            .lock()
            .unwrap()
            .spans
            .iter()
            .filter(|span| span.name == name)
            .cloned()
            .collect()
    }

    /// The recorded value of `field` on the first span called `span_name`.
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

impl SpanRecord {
    /// The span's name, for example `rpc`.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The recorded value of `field`, or `None` if it was never recorded
    /// (including when it was declared `Empty` and never filled in).
    pub fn field(&self, field: &str) -> Option<&str> {
        self.fields.get(field).map(String::as_str)
    }

    /// Whether `field` has a recorded value.
    pub fn has_field(&self, field: &str) -> bool {
        self.fields.contains_key(field)
    }
}

/// Install a capturing subscriber for the current thread.
///
/// Spans are captured until the returned guard is dropped, so keep it alive
/// for the whole test.
pub fn capture_spans() -> (SpanCapture, DefaultGuard) {
    let capture = SpanCapture::default();
    let subscriber = tracing_subscriber::registry().with(capture.clone());
    (capture, tracing::subscriber::set_default(subscriber))
}

#[derive(Default)]
struct State {
    spans: Vec<SpanRecord>,
    /// Open span ids to their index in `spans`. The registry reuses ids after
    /// a span closes, so entries are removed on close.
    open: HashMap<u64, usize>,
}

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
