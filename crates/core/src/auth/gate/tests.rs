//! Tests for the access gate's decision and its `Service` wrapper. No server
//! is needed: the decision is a plain function and the wrapper is driven with
//! `tower::ServiceExt`.

use super::*;

use std::convert::Infallible;
use std::sync::atomic::{AtomicUsize, Ordering};

use rust_auth_lib::KeyValidator;
use tonic::Code;
use tonic::codegen::http::HeaderValue;
use tower::ServiceExt;

use crate::auth::policy::{MethodPolicy, ServicePolicy};
use crate::testing::{SpanCapture, capture_spans, test_token, test_validator};

const KID: &str = "gate-kid";

struct Impl;
struct Server;

impl ServicePolicy<Server> for Impl {
    const POLICIES: &'static [MethodPolicy] = &[
        MethodPolicy::new("get_end_point_one", Access::any_of(&["operator"])),
        MethodPolicy::new("get_all", Access::all_of(&["operator", "admin"])),
        MethodPolicy::new("any_caller", Access::Authenticated),
        MethodPolicy::new("ping", Access::Public),
    ];
}

fn gate<S>(inner: S) -> AuthGate<S, KeyValidator> {
    AuthGate::new(
        inner,
        PolicyTable::of::<Server, Impl>(),
        test_validator(KID),
    )
}

fn token(roles: &[&str]) -> String {
    test_token(KID, "alice", roles)
}

fn headers(token: Option<&str>) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Some(token) = token {
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
    }
    headers
}

fn admit(
    method: &str,
    token: Option<&str>,
) -> (
    Result<Option<Caller>, Status>,
    Option<RpcMethod>,
    SpanCapture,
) {
    let (capture, _guard) = capture_spans();
    let rpc = RpcMethod::from_path(&format!("/pkg.Service/{method}"));
    let (span, _) = RpcSpan::open("/pkg.Service/Method");
    let outcome = gate(()).admit(rpc.as_ref(), &headers(token), None, &span);
    (outcome, rpc, capture)
}

fn code(outcome: &Result<Option<Caller>, Status>) -> Option<Code> {
    outcome.as_ref().err().map(Status::code)
}

#[test]
fn a_public_method_passes_with_no_token_and_no_caller() {
    let (outcome, ..) = admit("ping", None);

    assert!(matches!(outcome, Ok(None)));
}

#[test]
fn a_public_method_ignores_a_token_even_a_bad_one() {
    let (outcome, ..) = admit("ping", Some("not-a-jwt"));

    assert!(
        matches!(outcome, Ok(None)),
        "a public method never reads the token"
    );
}

#[test]
fn an_authenticated_method_needs_a_token() {
    let (outcome, ..) = admit("anyCaller", None);

    assert_eq!(code(&outcome), Some(Code::Unauthenticated));
}

#[test]
fn a_malformed_token_is_unauthenticated() {
    let (outcome, ..) = admit("anyCaller", Some("not-a-jwt"));

    assert_eq!(code(&outcome), Some(Code::Unauthenticated));
}

#[test]
fn a_valid_token_without_roles_passes_an_authenticated_method() {
    let (outcome, ..) = admit("anyCaller", Some(&token(&[])));

    let caller = outcome.unwrap().expect("a caller is attached");
    assert_eq!(rust_auth_lib::Claims::sub(&caller.claims), "alice");
}

#[test]
fn a_missing_role_is_permission_denied() {
    let (outcome, ..) = admit("getEndPointOne", Some(&token(&["viewer"])));

    assert_eq!(code(&outcome), Some(Code::PermissionDenied));
    assert_eq!(
        outcome.err().unwrap().message(),
        "required role not present"
    );
}

#[test]
fn the_required_role_is_admitted() {
    let (outcome, ..) = admit("getEndPointOne", Some(&token(&["operator"])));

    assert!(matches!(outcome, Ok(Some(_))));
}

#[test]
fn all_of_denies_a_partial_set() {
    let (outcome, ..) = admit("getAll", Some(&token(&["operator"])));
    assert_eq!(code(&outcome), Some(Code::PermissionDenied));

    let (outcome, ..) = admit("getAll", Some(&token(&["operator", "admin"])));
    assert!(matches!(outcome, Ok(Some(_))));
}

#[test]
fn a_method_missing_from_the_table_is_denied_even_with_a_valid_token() {
    let (outcome, ..) = admit("deleteEverything", Some(&token(&["admin", "operator"])));

    assert_eq!(code(&outcome), Some(Code::PermissionDenied));
}

#[test]
fn an_unparseable_path_is_denied() {
    let (span, _) = RpcSpan::open("garbage");

    let outcome = gate(()).admit(None, &headers(Some(&token(&["operator"]))), None, &span);

    assert_eq!(code(&outcome), Some(Code::PermissionDenied));
}

#[test]
fn access_and_user_are_recorded_on_the_span_for_every_outcome() {
    for (method, token, access, user) in [
        ("ping", None, "public", None),
        (
            "anyCaller",
            Some(token(&[])),
            "authenticated",
            Some("alice"),
        ),
        (
            "getEndPointOne",
            Some(token(&["viewer"])),
            "roles",
            Some("alice"),
        ),
        ("getEndPointOne", None, "roles", None),
    ] {
        let (capture, _guard) = capture_spans();
        let rpc = RpcMethod::from_path(&format!("/pkg.Service/{method}"));
        let (span, _) = RpcSpan::open("/pkg.Service/Method");

        let _ = gate(()).admit(rpc.as_ref(), &headers(token.as_deref()), None, &span);

        assert_eq!(
            capture.field("rpc", "access").as_deref(),
            Some(access),
            "{method}"
        );
        assert_eq!(capture.field("rpc", "user").as_deref(), user, "{method}");
    }
}

// ---------------------------------------------------------------------------
// The Service wrapper
// ---------------------------------------------------------------------------

/// Counts the calls that reach it and remembers what it was handed.
#[derive(Clone, Default)]
struct Probe {
    calls: Arc<AtomicUsize>,
    seen: Arc<std::sync::Mutex<Vec<Seen>>>,
}

#[derive(Debug)]
struct Seen {
    authorization: bool,
    method: Option<RpcMethod>,
    caller: bool,
}

impl Service<Request<Body>> for Probe {
    type Response = Response<Body>;
    type Error = Infallible;
    type Future = Ready<Result<Response<Body>, Infallible>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: Request<Body>) -> Self::Future {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.seen.lock().unwrap().push(Seen {
            authorization: request.headers().contains_key(AUTHORIZATION),
            method: request.extensions().get::<RpcMethod>().cloned(),
            caller: request.extensions().get::<Caller>().is_some(),
        });
        ready(Ok(Response::new(Body::empty())))
    }
}

async fn send(probe: &Probe, method: &str, token: Option<&str>) -> Response<Body> {
    let mut builder = Request::builder().uri(format!("/pkg.Service/{method}"));
    if let Some(token) = token {
        builder = builder.header(AUTHORIZATION, format!("Bearer {token}"));
    }
    gate(probe.clone())
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

fn grpc_status(response: &Response<Body>) -> Code {
    Code::from_bytes(response.headers().get("grpc-status").unwrap().as_bytes())
}

#[tokio::test]
async fn a_rejected_call_never_reaches_the_handler() {
    let probe = Probe::default();

    let denied = send(&probe, "getEndPointOne", Some(&token(&["viewer"]))).await;
    let anonymous = send(&probe, "anyCaller", None).await;
    let unlisted = send(&probe, "deleteEverything", Some(&token(&["operator"]))).await;

    assert_eq!(grpc_status(&denied), Code::PermissionDenied);
    assert_eq!(grpc_status(&anonymous), Code::Unauthenticated);
    assert_eq!(grpc_status(&unlisted), Code::PermissionDenied);
    assert_eq!(probe.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn an_admitted_call_carries_the_method_and_the_caller() {
    let probe = Probe::default();

    send(&probe, "getEndPointOne", Some(&token(&["operator"]))).await;

    let seen = probe.seen.lock().unwrap();
    let seen = &seen[0];
    assert!(seen.caller);
    assert_eq!(seen.method.as_ref().unwrap().method, "getEndPointOne");
}

#[tokio::test]
async fn a_public_call_carries_the_method_but_no_caller() {
    let probe = Probe::default();

    send(&probe, "ping", None).await;

    let seen = probe.seen.lock().unwrap();
    assert!(!seen[0].caller);
    assert_eq!(seen[0].method.as_ref().unwrap().method, "ping");
}

#[tokio::test]
async fn the_authorization_header_is_removed_on_every_admitted_path() {
    let probe = Probe::default();

    send(&probe, "getEndPointOne", Some(&token(&["operator"]))).await;
    send(&probe, "anyCaller", Some(&token(&[]))).await;
    // A token sent to a public method must not reach its handler either.
    send(&probe, "ping", Some(&token(&["operator"]))).await;

    let seen = probe.seen.lock().unwrap();
    assert_eq!(seen.len(), 3);
    assert!(seen.iter().all(|seen| !seen.authorization), "{seen:?}");
}
