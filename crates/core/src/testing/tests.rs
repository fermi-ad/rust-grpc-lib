//! Tests for the claim-injection helpers in [`super`].

use super::*;

use crate::auth::{Claims, KeycloakClaims};

fn claims(roles: &[&str]) -> KeycloakClaims {
    roles
        .iter()
        .fold(KeycloakClaimsBuilder::new("test-sub"), |b, r| {
            b.realm_role(*r)
        })
        .build()
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
