//! Tests for the JWT interceptors — covers header forwarding and token extraction.

use super::*;

// -----------------------------------------------------------------------
// Test a: header is copied correctly into the outbound request
// -----------------------------------------------------------------------
#[test]
fn header_is_forwarded_to_outbound_request() {
    let mut incoming = Request::new(());
    incoming
        .metadata_mut()
        .insert("authorization", "Bearer test-token-value".parse().unwrap());

    let mut interceptor = ForwardedTokenInterceptor::from_request(&incoming);

    let outbound = interceptor
        .call(Request::new(()))
        .expect("call should succeed");

    let auth_value = outbound
        .metadata()
        .get("authorization")
        .expect("authorization header must be present in outbound request");

    assert_eq!(
        auth_value.to_str().unwrap(),
        "Bearer test-token-value",
        "outbound authorization header must match the incoming one"
    );
}

// -----------------------------------------------------------------------
// Test b: get_token() strips the "Bearer " prefix
// -----------------------------------------------------------------------
#[test]
fn get_token_strips_bearer_prefix() {
    let mut incoming = Request::new(());
    incoming
        .metadata_mut()
        .insert("authorization", "Bearer test-token-value".parse().unwrap());

    let interceptor = ForwardedTokenInterceptor::from_request(&incoming);

    let token = interceptor
        .get_token()
        .expect("get_token should succeed when Authorization header is present");

    assert_eq!(
        token, "test-token-value",
        "get_token must return the raw token without the 'Bearer ' prefix"
    );
}

// -----------------------------------------------------------------------
// Test c: missing header → get_token() returns Err
// -----------------------------------------------------------------------
#[test]
fn get_token_returns_error_when_header_is_missing() {
    let incoming = Request::new(());
    let interceptor = ForwardedTokenInterceptor::from_request(&incoming);

    let result = interceptor.get_token();

    assert!(
        result.is_err(),
        "get_token must return Err when no Authorization header is present"
    );
}

// -----------------------------------------------------------------------
// Test d: no header → call() passes through without inserting
// -----------------------------------------------------------------------
#[test]
fn call_does_not_insert_header_when_none_was_present() {
    let incoming = Request::new(());
    let mut interceptor = ForwardedTokenInterceptor::from_request(&incoming);

    let outbound = interceptor
        .call(Request::new(()))
        .expect("call should succeed even without an Authorization header");

    assert!(
        outbound.metadata().get("authorization").is_none(),
        "outbound request must not have an authorization header when none was forwarded"
    );
}
