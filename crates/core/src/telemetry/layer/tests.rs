//! Tests for tracking terminal status without changing the service or body output.

use super::*;

use std::collections::VecDeque;
use std::future::{pending, poll_fn, ready};
use std::sync::Mutex;
use std::task::Waker;

use tonic::Status;
use tonic::codegen::{Bytes, http::HeaderValue};
use tower::service_fn;

use crate::auth::{Access, MethodPolicy, PolicyTable, ServicePolicy};
use crate::telemetry::{CallSummary, TelemetryConfig};

const METHOD: &str = "pkg.Service/getValue";
const PATH: &str = "/pkg.Service/getValue";

/// Records completions seen by the call observer.
#[derive(Clone, Default)]
struct Collector(Arc<Mutex<Vec<CallSummary>>>);

impl Collector {
    fn assert_open(&self) {
        assert!(self.0.lock().unwrap().is_empty());
    }

    fn assert_finished(&self, method: Option<&'static str>, status: Code) {
        let seen = self.0.lock().unwrap();
        let [summary] = seen.as_slice() else {
            panic!("expected one completion, got {seen:?}");
        };
        assert_eq!((summary.method, summary.status), (method, status));
    }
}

#[derive(Default)]
struct TestBody {
    frames: VecDeque<Result<Frame<Bytes>, &'static str>>,
    // Some bodies know they are exhausted without a poll returning `None`.
    reports_end: bool,
    open: bool,
}

impl Body for TestBody {
    type Data = Bytes;
    type Error = &'static str;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Self::Error>>> {
        match self.frames.pop_front() {
            None if self.open => Poll::Pending,
            frame => Poll::Ready(frame),
        }
    }

    fn is_end_stream(&self) -> bool {
        self.reports_end && self.frames.is_empty()
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::with_exact(0)
    }
}

struct TestPolicy;

impl ServicePolicy<()> for TestPolicy {
    const POLICIES: &'static [MethodPolicy] = &[MethodPolicy::new("get_value", Access::Public)];
}

fn tracker() -> (CallLayer, Collector) {
    let mut labels = MethodLabelIndex::new();
    labels.register(
        "pkg.Service",
        &[METHOD],
        PolicyTable::of::<(), TestPolicy>(),
    );
    let sink = Collector::default();
    let observed = sink.clone();
    let layer = CallLayer::new(
        &Telemetry::start(TelemetryConfig::default()),
        labels,
        Some(Arc::new(move |summary| {
            observed.0.lock().unwrap().push(*summary)
        })),
        watch::channel(()).1,
    );
    (layer, sink)
}

fn request(path: &str) -> Request<()> {
    Request::builder().uri(path).body(()).unwrap()
}

fn trailers(status: Option<HeaderValue>) -> Frame<Bytes> {
    let mut headers = HeaderMap::new();
    if let Some(status) = status {
        headers.insert("grpc-status", status);
    }
    Frame::trailers(headers)
}

#[tokio::test]
async fn header_status_finishes_without_polling_the_empty_body_and_labels_are_exact() {
    for (path, status, method, expected) in [
        (PATH, "7", Some(METHOD), Code::PermissionDenied),
        ("/pkg.Service/GetValue", "0", None, Code::Ok),
        ("/pkg.Service/unlisted", "0", None, Code::Ok),
        (PATH, "not-a-code", Some(METHOD), Code::Unknown),
        (
            "/grpc.health.v1.Health/Check",
            "0",
            Some("grpc.health.v1.Health/Check"),
            Code::Ok,
        ),
    ] {
        let (layer, sink) = tracker();
        let mut service = layer.clone().layer(service_fn(move |_: Request<()>| {
            ready(
                Response::builder()
                    .header("grpc-status", status)
                    .body(TestBody {
                        reports_end: true,
                        ..TestBody::default()
                    }),
            )
        }));
        let response = service.call(request(path)).await.unwrap();
        assert_eq!(response.headers()["grpc-status"], status);
        assert!(response.body().is_end_stream());
        sink.assert_finished(method, expected);
        drop(response);
        sink.assert_finished(method, expected);
    }
}

#[tokio::test]
async fn streaming_waits_for_trailers_and_forwards_frames() {
    for (status, expected) in [
        (Some(HeaderValue::from_static("0")), Code::Ok),
        (Some(HeaderValue::from_static("13")), Code::Internal),
        (
            Some(HeaderValue::from_bytes(b"\xff").unwrap()),
            Code::Unknown,
        ),
        (None, Code::Unknown),
    ] {
        let (layer, sink) = tracker();
        let mut service = layer.layer(service_fn(|_: Request<()>| {
            ready(Ok::<_, Status>(Response::new(TestBody {
                frames: [
                    Ok(Frame::data(Bytes::from_static(b"data"))),
                    Ok(trailers(status.clone())),
                ]
                .into(),
                ..TestBody::default()
            })))
        }));
        let mut body = service.call(request(PATH)).await.unwrap().into_body();
        sink.assert_open();
        assert!(!body.is_end_stream());
        assert_eq!(body.size_hint().exact(), Some(0));

        let data = poll_fn(|cx| Pin::new(&mut body).poll_frame(cx))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(data.data_ref().unwrap(), b"data".as_slice());
        sink.assert_open();

        let frame = poll_fn(|cx| Pin::new(&mut body).poll_frame(cx))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            frame.trailers_ref().unwrap().get("grpc-status"),
            status.as_ref()
        );
        sink.assert_finished(Some(METHOD), expected);
        assert!(
            poll_fn(|cx| Pin::new(&mut body).poll_frame(cx))
                .await
                .is_none()
        );
        drop(body);
        sink.assert_finished(Some(METHOD), expected);
    }
}

#[tokio::test]
async fn exhausted_body_without_status_is_unknown_even_without_a_final_none_poll() {
    for (reports_end, with_data) in [(true, false), (true, true), (false, false)] {
        let (layer, sink) = tracker();
        let mut service = layer.layer(service_fn(|_: Request<()>| {
            ready(Ok::<_, Status>(Response::new(TestBody {
                frames: with_data
                    .then(|| Ok(Frame::data(Bytes::from_static(b"last data"))))
                    .into_iter()
                    .collect(),
                reports_end,
                open: false,
            })))
        }));
        let mut body = service.call(request(PATH)).await.unwrap().into_body();
        if !body.is_end_stream() {
            sink.assert_open();
            let frame = poll_fn(|cx| Pin::new(&mut body).poll_frame(cx)).await;
            assert_eq!(frame.is_some(), with_data);
        }
        sink.assert_finished(Some(METHOD), Code::Unknown);
        drop(body);
        sink.assert_finished(Some(METHOD), Code::Unknown);
    }
}

#[tokio::test]
async fn service_and_body_errors_are_forwarded_with_unknown_status() {
    let (layer, sink) = tracker();
    let mut service = layer.layer(service_fn(|_: Request<()>| {
        ready(Err::<Response<TestBody>, _>("service failure"))
    }));
    assert_eq!(
        service.call(request(PATH)).await.err(),
        Some("service failure")
    );
    sink.assert_finished(Some(METHOD), Code::Unknown);

    let (layer, sink) = tracker();
    let mut service = layer.layer(service_fn(|_: Request<()>| {
        ready(Ok::<_, Status>(Response::new(TestBody {
            frames: [Err("body failure")].into(),
            ..TestBody::default()
        })))
    }));
    let mut body = service.call(request(PATH)).await.unwrap().into_body();
    sink.assert_open();
    let error = poll_fn(|cx| Pin::new(&mut body).poll_frame(cx))
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(error, "body failure");
    sink.assert_finished(Some(METHOD), Code::Unknown);
    drop(body);
    sink.assert_finished(Some(METHOD), Code::Unknown);
}

#[test]
fn dropping_an_unpolled_or_pending_future_cancels() {
    let mut cx = Context::from_waker(Waker::noop());
    for poll_first in [false, true] {
        let (layer, sink) = tracker();
        let mut service = layer.layer(service_fn(|_: Request<()>| {
            pending::<Result<Response<TestBody>, Status>>()
        }));
        let mut future = Box::pin(service.call(request(PATH)));
        if poll_first {
            assert!(future.as_mut().poll(&mut cx).is_pending());
        }
        sink.assert_open();
        drop(future);
        sink.assert_finished(Some(METHOD), Code::Cancelled);
    }
}

#[tokio::test]
async fn dropping_an_unpolled_or_pending_stream_cancels() {
    let mut cx = Context::from_waker(Waker::noop());
    for poll_first in [false, true] {
        let (layer, sink) = tracker();
        let mut service = layer.layer(service_fn(|_: Request<()>| {
            ready(Ok::<_, Status>(Response::new(TestBody {
                frames: [Ok(Frame::data(Bytes::from_static(b"first data")))].into(),
                open: true,
                ..TestBody::default()
            })))
        }));
        let mut body = service.call(request(PATH)).await.unwrap().into_body();
        if poll_first {
            assert!(Pin::new(&mut body).poll_frame(&mut cx).is_ready());
            assert!(Pin::new(&mut body).poll_frame(&mut cx).is_pending());
        }
        sink.assert_open();
        drop(body);
        sink.assert_finished(Some(METHOD), Code::Cancelled);
    }
}

#[test]
fn randomized_poll_and_drop_orders_finish_once_per_start() {
    let mut cx = Context::from_waker(Waker::noop());
    let mut seed = 42_u32;
    let mut next_poll_count = || {
        // Deterministic pseudo-random choices, without a test dependency.
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (seed >> 16) % 5
    };
    for _ in 0..1000 {
        let (layer, sink) = tracker();
        let mut service = layer.layer(service_fn(|_: Request<()>| {
            let mut first = true;
            poll_fn(move |_| {
                if std::mem::take(&mut first) {
                    Poll::Pending
                } else {
                    Poll::Ready(Ok::<_, Status>(Response::new(TestBody {
                        frames: [
                            Ok(Frame::data(Bytes::from_static(b"first"))),
                            Ok(trailers(Some(HeaderValue::from_static("0")))),
                        ]
                        .into(),
                        ..TestBody::default()
                    })))
                }
            })
        }));
        let mut future = Box::pin(service.call(request(PATH)));
        let mut expected = Code::Cancelled;
        let response = (0..next_poll_count()).find_map(|_| match future.as_mut().poll(&mut cx) {
            Poll::Ready(result) => Some(result.unwrap()),
            Poll::Pending => None,
        });
        drop(future);
        if let Some(response) = response {
            sink.assert_open();
            let mut body = response.into_body();
            for _ in 0..next_poll_count() {
                if let Poll::Ready(Some(Ok(frame))) = Pin::new(&mut body).poll_frame(&mut cx)
                    && frame.is_trailers()
                {
                    expected = Code::Ok;
                }
            }
            drop(body);
        }
        sink.assert_finished(Some(METHOD), expected);
    }
}
