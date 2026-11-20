//! Tests for [`super::CallInfo`].

use super::*;

use std::net::{IpAddr, Ipv4Addr};

use tonic::transport::server::TcpConnectInfo;

use crate::auth::context::tests::{authenticated, public};
use crate::testing::capture_spans;

#[test]
fn remote_addr_is_captured_from_connect_info() {
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 1, 2, 3)), 4242);
    let mut req = authenticated((), &[]);
    req.extensions_mut().insert(TcpConnectInfo {
        local_addr: None,
        remote_addr: Some(addr),
    });

    let (info, ()) = CallInfo::from_request(req).unwrap();

    assert_eq!(info.remote_addr, Some(addr));
}

#[test]
fn the_span_is_the_rpc_span_from_the_extensions() {
    let (capture, _guard) = capture_spans();
    let (span, _method) = RpcSpan::open("/pkg.Service/Do");
    let mut req = authenticated((), &[]);
    req.extensions_mut().insert(span);

    let (info, ()) = CallInfo::from_request(req).unwrap();
    info.span().record("grpc.status", "Ok");

    assert_eq!(capture.field("rpc", "grpc.status").as_deref(), Some("Ok"));
}

#[test]
fn without_a_trace_layer_the_span_is_disabled() {
    let (info, ()) = CallInfo::from_request(public(())).unwrap();

    assert!(info.span().is_disabled());
}

#[tokio::test]
async fn instrument_runs_a_spawned_future_inside_the_rpc_span() {
    let (capture, _guard) = capture_spans();
    let (span, _method) = RpcSpan::open("/pkg.Service/Do");
    let mut req = authenticated((), &[]);
    req.extensions_mut().insert(span);
    let (info, ()) = CallInfo::from_request(req).unwrap();

    // Without the helper a spawned task has no parent; with it, it is in the
    // rpc span.
    let rpc_id = info.span().id();
    assert!(rpc_id.is_some(), "the rpc span is enabled under capture");
    // Called as an associated function: `tracing::Instrument` is in scope here
    // and would win the method-call resolution.
    let inside = CallInfo::instrument(&info, async { Span::current().id() }).await;
    let outside = Instrument::instrument(async { Span::current().id() }, Span::none()).await;

    assert_eq!(inside, rpc_id);
    assert_ne!(outside, rpc_id);
    drop(capture);
}
