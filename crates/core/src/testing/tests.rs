//! Tests for the claim helpers and span capture in [`super`].

use super::*;

use crate::auth::Claims;

fn claims(roles: &[&str]) -> KeycloakClaims {
    claims_with_roles("test-sub", roles)
}

#[test]
fn claims_with_roles_sets_the_subject_and_every_role() {
    let claims = claims_with_roles("alice", &["operator", "admin"]);

    assert_eq!(claims.sub(), "alice");
    assert!(claims.has_role("operator"));
    assert!(claims.has_role("admin"));
    assert!(!claims.has_role("viewer"));
}

#[test]
fn claims_with_no_roles_has_none() {
    assert!(!claims_with_roles("alice", &[]).has_role("operator"));
}

#[test]
fn capture_spans_records_initial_and_later_values() {
    let (capture, _guard) = capture_spans();

    let span = tracing::info_span!("work", kind = "initial", later = tracing::field::Empty);
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

    drop(tracing::info_span!("short", n = 1));
    drop(tracing::info_span!("short", n = 2));

    let records = capture.spans("short");
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].field("n"), Some("1"));
    assert_eq!(records[1].field("n"), Some("2"));
}

#[test]
fn with_claims_inserts_claims_into_the_extensions() {
    let request = with_claims(Request::new("hello"), claims(&["operator"]));

    let found = request
        .extensions()
        .get::<KeycloakClaims>()
        .expect("claims were inserted");
    assert_eq!(found.sub(), "test-sub");
    assert!(found.has_role("operator"));
    assert_eq!(*request.get_ref(), "hello");
}

#[test]
fn with_claims_keeps_metadata_and_message() {
    let mut request = Request::new(7_u32);
    request
        .metadata_mut()
        .insert("x-trace", "abc".parse().unwrap());

    let request = with_claims(request, claims(&[]));

    assert_eq!(request.metadata().get("x-trace").unwrap(), "abc");
    assert_eq!(*request.get_ref(), 7);
}

#[test]
fn request_with_claims_wraps_the_message() {
    let request = request_with_claims("msg".to_string(), claims(&["admin"]));

    assert_eq!(request.get_ref(), "msg");
    assert!(
        request
            .extensions()
            .get::<KeycloakClaims>()
            .unwrap()
            .has_role("admin")
    );
}

#[test]
fn injected_claims_are_readable_through_request_context() {
    let request = request_with_claims((), claims(&["operator"]));

    let ctx =
        crate::auth::RequestContext::<()>::try_from_request(request).expect("claims were injected");

    assert!(ctx.has_role("operator"));
}
