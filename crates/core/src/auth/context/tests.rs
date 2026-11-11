//! Tests for [`super::RequestContext`], plus the request fixtures shared with
//! the tests of [`super::CallInfo`] and [`super::PublicContext`].

use super::*;

use crate::testing::claims_with_roles;
use rust_auth_lib::TokenProvider;
use tonic::Code;

pub(super) const TOKEN: &str = "eyJhbGciOi.secret-payload.signature";

/// Extension used to prove that arbitrary extensions pass through.
#[derive(Clone, Debug, PartialEq)]
struct Marker(u8);

pub(super) fn method() -> RpcMethod {
    RpcMethod {
        service: "pkg.Service".into(),
        method: "Do".into(),
    }
}

/// A request the way the gate leaves it for an authenticated method.
pub(super) fn authenticated<T>(message: T, roles: &[&str]) -> Request<T> {
    let mut request = Request::new(message);
    request
        .extensions_mut()
        .insert(Caller::new(claims_with_roles("test-sub", roles), TOKEN));
    request.extensions_mut().insert(method());
    request
}

/// A request the way the gate leaves it for a public method.
pub(super) fn public<T>(message: T) -> Request<T> {
    let mut request = Request::new(message);
    request.extensions_mut().insert(method());
    request
}

#[test]
fn a_caller_builds_the_context() {
    let ctx = RequestContext::<String>::try_from_request(authenticated(
        "hello".to_string(),
        &["operator"],
    ))
    .expect("the caller is present");

    assert_eq!(ctx.message, "hello");
    assert_eq!(ctx.sub(), "test-sub");
    assert!(ctx.has_role("operator"));
    assert!(!ctx.has_role("admin"));
    assert!(ctx.remote_addr.is_none());
    assert_eq!(ctx.rpc, method());
}

#[test]
fn a_missing_caller_fails_closed_with_a_generic_message() {
    let err = RequestContext::<String>::try_from_request(public("hello".to_string()))
        .expect_err("must fail closed without a caller");

    assert_eq!(err.code(), Code::Internal);
    assert_eq!(err.message(), "authentication is not configured");
    assert!(
        !err.message().contains("gate") && !err.message().contains("Keycloak"),
        "the misconfiguration detail must not reach the remote client"
    );
}

#[test]
fn bare_claims_in_the_extensions_count_as_missing() {
    let mut req = public(());
    req.extensions_mut()
        .insert(claims_with_roles("mallory", &["admin"]));

    let err = RequestContext::<()>::try_from_request(req)
        .expect_err("only a caller the gate verified is accepted");

    assert_eq!(err.code(), Code::Internal);
}

#[test]
fn metadata_and_extensions_pass_through() {
    let mut req = authenticated((), &[]);
    req.metadata_mut()
        .insert("x-request-id", "abc-123".parse().unwrap());
    req.extensions_mut().insert(Marker(7));

    let ctx = RequestContext::<()>::try_from_request(req).unwrap();

    assert_eq!(ctx.metadata.get("x-request-id").unwrap(), "abc-123");
    assert_eq!(ctx.extensions.get::<Marker>(), Some(&Marker(7)));
}

#[test]
fn the_caller_is_not_left_in_the_extensions() {
    let ctx = RequestContext::<()>::try_from_request(authenticated((), &[])).unwrap();

    assert!(
        ctx.extensions.get::<Caller>().is_none(),
        "the token must not stay reachable (or printable) through the extensions"
    );
}

#[test]
fn token_returns_the_callers_token_as_a_provider() {
    let ctx = RequestContext::<()>::try_from_request(authenticated((), &[])).unwrap();

    assert_eq!(ctx.token().get_token().unwrap(), TOKEN);
}

#[test]
fn forward_sets_the_authorization_header_and_keeps_the_message() {
    let ctx = RequestContext::<()>::try_from_request(authenticated((), &[])).unwrap();

    let request = ctx.forward("outbound".to_string());

    assert_eq!(request.get_ref(), "outbound");
    let header = request.metadata().get("authorization").unwrap();
    assert_eq!(header.to_str().unwrap(), format!("Bearer {TOKEN}"));
}

#[test]
fn into_forwarded_moves_the_message_and_sets_a_sensitive_header() {
    let ctx = RequestContext::try_from_request(authenticated(vec![1_u8, 2, 3], &[])).unwrap();

    let request = ctx.into_forwarded();

    assert_eq!(request.get_ref(), &[1, 2, 3]);
    let header = request.metadata().get("authorization").unwrap();
    assert_eq!(header.to_str().unwrap(), format!("Bearer {TOKEN}"));
    assert!(header.is_sensitive());
    let shown = format!("{:?}", request.metadata());
    assert!(!shown.contains(TOKEN), "{shown}");
}

#[test]
fn into_forwarded_carries_no_other_metadata() {
    let mut req = authenticated((), &[]);
    req.metadata_mut()
        .insert("x-request-id", "abc-123".parse().unwrap());
    let ctx = RequestContext::<()>::try_from_request(req).unwrap();

    let request = ctx.into_forwarded();

    assert!(request.metadata().get("x-request-id").is_none());
    assert_eq!(request.metadata().len(), 1);
}

#[test]
fn request_context_debug_never_contains_the_token() {
    let mut metadata = tonic::metadata::MetadataMap::new();
    // Even a header that somehow stayed in the metadata must not be printed:
    // the gate removes it, and `forward` marks its own copy sensitive.
    metadata.insert("x-trace", "visible".parse().unwrap());
    let ctx =
        RequestContext::<String>::try_from_request(authenticated("msg".to_string(), &["operator"]))
            .unwrap()
            .with_metadata(metadata);

    let shown = format!("{ctx:?}");

    assert!(!shown.contains(TOKEN), "{shown}");
    assert!(!shown.contains("secret-payload"), "{shown}");
    assert!(shown.contains("redacted"), "{shown}");
    assert!(shown.contains("visible"), "{shown}");
}
