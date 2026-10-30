//! Tests for [`super::RequestContext`].

use super::*;

use std::net::{IpAddr, Ipv4Addr};

use rust_auth_lib::test_fixtures::KeycloakClaimsBuilder;
use tonic::{
    Code, Streaming,
    body::Body,
    codec::{DecodeBuf, Decoder},
    transport::server::TcpConnectInfo,
};

/// Claims type that is not the one the tests ask for.
#[derive(Clone)]
struct OtherClaims;

/// Extension used to prove that arbitrary extensions pass through.
#[derive(Clone, Debug, PartialEq)]
struct Marker(u8);

/// Decoder that never yields a message, enough to build an empty `Streaming`.
struct NoopDecoder;

impl Decoder for NoopDecoder {
    type Item = u32;
    type Error = Status;

    fn decode(&mut self, _src: &mut DecodeBuf<'_>) -> Result<Option<u32>, Status> {
        Ok(None)
    }
}

fn claims(roles: &[&str]) -> KeycloakClaims {
    roles
        .iter()
        .fold(KeycloakClaimsBuilder::new("test-sub"), |b, r| {
            b.realm_role(*r)
        })
        .build()
}

fn request_with_claims<T>(message: T, roles: &[&str]) -> Request<T> {
    let mut req = Request::new(message);
    req.extensions_mut().insert(claims(roles));
    req
}

#[test]
fn claims_present_builds_context() {
    let req = request_with_claims("hello".to_string(), &["operator"]);

    let ctx = RequestContext::<String>::try_from_request(req).expect("claims are present");

    assert_eq!(ctx.message, "hello");
    assert_eq!(ctx.sub(), "test-sub");
    assert!(ctx.has_role("operator"));
    assert!(!ctx.has_role("admin"));
    assert!(ctx.remote_addr.is_none());
    assert!(ctx.rpc_method.is_none());
}

#[test]
fn missing_claims_returns_generic_internal_error() {
    let req = Request::new("hello".to_string());

    let err = RequestContext::<String>::try_from_request(req)
        .expect_err("must fail closed without claims");

    assert_eq!(err.code(), Code::Internal);
    assert_eq!(err.message(), "authentication is not configured");
    assert!(
        !err.message().contains("Layer") && !err.message().contains("Keycloak"),
        "the misconfiguration detail must not reach the remote client"
    );
}

#[test]
fn claims_of_a_different_type_count_as_missing() {
    let mut req = Request::new(());
    req.extensions_mut().insert(OtherClaims);

    let err = RequestContext::<(), KeycloakClaims>::try_from_request(req)
        .expect_err("only claims of type C are accepted");

    assert_eq!(err.code(), Code::Internal);
}

#[test]
fn streaming_message_type_is_supported() {
    let stream = Streaming::new_empty(NoopDecoder, Body::empty());
    let req = request_with_claims(stream, &["viewer"]);

    let ctx: RequestContext<Streaming<u32>> =
        RequestContext::try_from_request(req).expect("claims are present");

    assert!(ctx.has_role("viewer"));
    // The stream itself is handed to the handler untouched.
    let _stream: Streaming<u32> = ctx.message;
}

#[test]
fn metadata_and_extensions_pass_through() {
    let mut req = request_with_claims((), &[]);
    req.metadata_mut()
        .insert("x-request-id", "abc-123".parse().unwrap());
    req.extensions_mut().insert(Marker(7));

    let ctx = RequestContext::<()>::try_from_request(req).unwrap();

    assert_eq!(ctx.metadata.get("x-request-id").unwrap(), "abc-123");
    assert_eq!(ctx.extensions.get::<Marker>(), Some(&Marker(7)));
    // Claims are cloned out, not removed.
    assert!(ctx.extensions.get::<KeycloakClaims>().is_some());
}

#[test]
fn remote_addr_is_captured_from_connect_info() {
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 1, 2, 3)), 4242);
    let mut req = request_with_claims((), &[]);
    req.extensions_mut().insert(TcpConnectInfo {
        local_addr: None,
        remote_addr: Some(addr),
    });

    let ctx = RequestContext::<()>::try_from_request(req).unwrap();

    assert_eq!(ctx.remote_addr, Some(addr));
}

#[test]
fn rpc_method_extension_is_surfaced() {
    let method = RpcMethod {
        service: "pkg.Service".into(),
        method: "Do".into(),
    };
    let mut req = request_with_claims((), &[]);
    req.extensions_mut().insert(method.clone());

    let ctx = RequestContext::<()>::try_from_request(req).unwrap();

    assert_eq!(ctx.rpc_method, Some(method));
}

#[test]
fn for_test_builds_a_context_shaped_like_production() {
    let ctx = RequestContext::for_test(claims(&["operator"]), "payload".to_string());

    assert_eq!(ctx.message, "payload");
    assert_eq!(ctx.sub(), "test-sub");
    assert!(ctx.has_role("operator"));
    assert!(ctx.metadata.is_empty());
    assert!(ctx.remote_addr.is_none());
    assert!(ctx.rpc_method.is_none());
    assert!(ctx.extensions.get::<KeycloakClaims>().is_some());
}

#[test]
fn expiry_accessors_reflect_the_claims() {
    let live = RequestContext::<()>::try_from_request(request_with_claims((), &[])).unwrap();
    assert!(!live.is_expired());
    assert!(live.exp_unix() > 0);

    let mut req = Request::new(());
    req.extensions_mut()
        .insert(KeycloakClaimsBuilder::new("old").iat(1).exp(2).build());
    let expired = RequestContext::<()>::try_from_request(req).unwrap();
    assert!(expired.is_expired());
    assert_eq!(expired.exp_unix(), 2);
}
