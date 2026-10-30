//! Tests for [`super::GrpcTraceLayer`].

use super::*;

use std::convert::Infallible;
use std::sync::{Arc, Mutex};

use http_body::{Body as HttpBody, Frame};
use tonic::Status;
use tonic::body::Body;
use tonic::codegen::{
    Bytes,
    http::{self, HeaderMap, HeaderValue, Response},
};
use tower::service_fn;

use crate::auth::RpcMethod;
use crate::testing::capture_spans;

/// What a test saw the layer put into the request extensions.
#[derive(Default)]
struct Seen {
    method: Option<RpcMethod>,
    has_span: bool,
}

/// A response body that ends with a `grpc-status` trailer, like a streaming RPC.
struct TrailersBody(Option<HeaderMap>);

impl HttpBody for TrailersBody {
    type Data = Bytes;
    type Error = Status;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Status>>> {
        Poll::Ready(self.0.take().map(|trailers| Ok(Frame::trailers(trailers))))
    }
}

/// A response body that never produces a frame, like a stream that stays open.
struct OpenBody;

impl HttpBody for OpenBody {
    type Data = Bytes;
    type Error = Status;

    fn poll_frame(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Status>>> {
        Poll::Pending
    }
}

fn request(path: &str) -> http::Request<Body> {
    http::Request::builder()
        .uri(path)
        .body(Body::empty())
        .unwrap()
}

/// A service that records what the layer inserted, then answers with `grpc-status`.
fn recording_service(
    seen: Arc<Mutex<Seen>>,
    grpc_status: &'static str,
) -> impl Service<
    http::Request<Body>,
    Response = Response<Body>,
    Error = Infallible,
    Future = impl Future<Output = Result<Response<Body>, Infallible>>,
> {
    service_fn(move |request: http::Request<Body>| {
        let seen = Arc::clone(&seen);
        async move {
            let mut seen = seen.lock().unwrap();
            seen.method = request.extensions().get::<RpcMethod>().cloned();
            seen.has_span = request.extensions().get::<RpcSpan>().is_some();
            Ok(Response::builder()
                .header("grpc-status", grpc_status)
                .body(Body::empty())
                .unwrap())
        }
    })
}

#[tokio::test]
async fn layer_inserts_the_method_and_span_extensions() {
    let (_capture, _guard) = capture_spans();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let mut service = GrpcTraceLayer::new().layer(recording_service(Arc::clone(&seen), "0"));

    service.call(request("/pkg.Service/Method")).await.unwrap();

    let seen = seen.lock().unwrap();
    assert_eq!(
        seen.method,
        Some(RpcMethod {
            service: "pkg.Service".into(),
            method: "Method".into(),
        })
    );
    assert!(seen.has_span);
}

#[tokio::test]
async fn span_carries_the_rpc_fields() {
    let (capture, _guard) = capture_spans();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let mut service = GrpcTraceLayer::new().layer(recording_service(seen, "0"));

    service.call(request("/pkg.Service/Method")).await.unwrap();

    assert_eq!(capture.field("rpc", "rpc.system").as_deref(), Some("grpc"));
    assert_eq!(
        capture.field("rpc", "rpc.service").as_deref(),
        Some("pkg.Service")
    );
    assert_eq!(
        capture.field("rpc", "rpc.method").as_deref(),
        Some("Method")
    );
    assert!(capture.field("rpc", "latency_ms").is_some());
}

#[tokio::test]
async fn grpc_status_of_the_response_is_recorded() {
    let (capture, _guard) = capture_spans();
    let seen = Arc::new(Mutex::new(Seen::default()));
    // 7 is `PermissionDenied`; a trailers-only response carries it in the headers.
    let mut service = GrpcTraceLayer::new().layer(recording_service(seen, "7"));

    service.call(request("/pkg.Service/Method")).await.unwrap();

    assert_eq!(
        capture.field("rpc", "grpc.status").as_deref(),
        Some("PermissionDenied")
    );
}

#[tokio::test]
async fn unparseable_path_is_marked_unknown_and_has_no_method() {
    let (capture, _guard) = capture_spans();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let mut service = GrpcTraceLayer::new().layer(recording_service(Arc::clone(&seen), "0"));

    service.call(request("/not-a-grpc-path")).await.unwrap();

    assert_eq!(
        capture.field("rpc", "rpc.service").as_deref(),
        Some("unknown")
    );
    assert_eq!(
        capture.field("rpc", "rpc.method").as_deref(),
        Some("unknown")
    );
    let seen = seen.lock().unwrap();
    assert_eq!(seen.method, None);
    assert!(seen.has_span, "the span is published even without a method");
}

#[tokio::test]
async fn dropped_future_is_recorded_as_cancelled() {
    let (capture, _guard) = capture_spans();
    let pending = service_fn(|_request: http::Request<Body>| {
        std::future::pending::<Result<Response<Body>, Infallible>>()
    });
    let mut service = GrpcTraceLayer::new().layer(pending);

    // What tonic's timeout does when the deadline passes.
    let future = service.call(request("/pkg.Service/Method"));
    drop(future);

    assert_eq!(
        capture.field("rpc", "grpc.status").as_deref(),
        Some("Cancelled")
    );
}

#[tokio::test]
async fn completed_future_is_not_marked_cancelled() {
    let (capture, _guard) = capture_spans();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let mut service = GrpcTraceLayer::new().layer(recording_service(seen, "0"));

    let response = service.call(request("/pkg.Service/Method")).await.unwrap();
    drop(response);

    assert_eq!(capture.field("rpc", "grpc.status").as_deref(), Some("Ok"));
}

#[tokio::test]
async fn grpc_status_in_trailers_is_recorded_when_the_stream_ends() {
    let (capture, _guard) = capture_spans();
    let streaming = service_fn(|_request: http::Request<Body>| async {
        // No `grpc-status` header: the status only arrives in the trailers.
        let mut trailers = HeaderMap::new();
        trailers.insert("grpc-status", HeaderValue::from_static("13"));
        Ok::<_, Infallible>(Response::new(Body::new(TrailersBody(Some(trailers)))))
    });
    let mut service = GrpcTraceLayer::new().layer(streaming);

    let response = service.call(request("/pkg.Service/Stream")).await.unwrap();
    assert_eq!(
        capture.field("rpc", "grpc.status"),
        None,
        "the status is not known until the stream ends"
    );

    let mut body = std::pin::pin!(response.into_body());
    while std::future::poll_fn(|cx| body.as_mut().poll_frame(cx))
        .await
        .is_some()
    {}

    assert_eq!(
        capture.field("rpc", "grpc.status").as_deref(),
        Some("Internal")
    );
    assert!(capture.field("rpc", "stream_ms").is_some());
}

/// Documents the gap described in the module docs: the cancel guard only
/// covers the response *future*, so a client that cancels after the headers
/// were sent leaves the status unset.
#[tokio::test]
async fn cancelling_mid_stream_leaves_the_status_unset() {
    let (capture, _guard) = capture_spans();
    let streaming = service_fn(|_request: http::Request<Body>| async {
        Ok::<_, Infallible>(Response::new(Body::new(OpenBody)))
    });
    let mut service = GrpcTraceLayer::new().layer(streaming);

    let response = service.call(request("/pkg.Service/Stream")).await.unwrap();
    drop(response);

    assert!(capture.field("rpc", "latency_ms").is_some());
    assert_eq!(capture.field("rpc", "grpc.status"), None);
    assert_eq!(capture.field("rpc", "stream_ms"), None);
}
