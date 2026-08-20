//! Tests for [`super::extract_token`].

use super::*;

#[test]
fn extract_token_strips_bearer_prefix() {
    let mut incoming = Request::new(());
    incoming
        .metadata_mut()
        .insert("authorization", "Bearer test-token-value".parse().unwrap());

    let provider = extract_token(&incoming).unwrap();

    let token = provider
        .get_token()
        .expect("get_token should succeed when Authorization header is present");

    assert_eq!(
        token, "test-token-value",
        "get_token must return the raw token without the 'Bearer ' prefix"
    );
}

#[test]
fn extract_token_returns_error_when_header_is_missing() {
    let incoming = Request::new(());
    let result = extract_token(&incoming);

    assert!(
        result.is_err(),
        "get_token must return Err when no Authorization header is present"
    );
}
