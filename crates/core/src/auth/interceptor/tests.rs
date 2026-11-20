//! Tests for [`super::ClientJwtInterceptor`].

use super::*;

use rust_auth_lib::{ForwardedToken, TokenError};
use tonic::Code;

const TOKEN: &str = "eyJhbGciOi.secret-payload.signature";

/// A provider that cannot produce a token.
struct Failing;

impl TokenProvider for Failing {
    fn get_token(&self) -> Result<String, TokenError> {
        Err(TokenError::CacheExpired)
    }
}

fn call<P: TokenProvider>(provider: P) -> Result<Request<()>, Status> {
    ClientJwtInterceptor::new(provider).call(Request::new(()))
}

#[test]
fn the_token_is_attached_as_a_bearer_header() {
    let request = call(ForwardedToken::new(TOKEN)).unwrap();

    let header = request.metadata().get("authorization").unwrap();
    assert_eq!(header.to_str().unwrap(), format!("Bearer {TOKEN}"));
}

#[test]
fn the_header_is_marked_sensitive() {
    let request = call(ForwardedToken::new(TOKEN)).unwrap();

    assert!(
        request
            .metadata()
            .get("authorization")
            .unwrap()
            .is_sensitive()
    );
    let shown = format!("{:?}", request.metadata());
    assert!(!shown.contains(TOKEN), "{shown}");
}

#[test]
fn a_provider_failure_is_unauthenticated() {
    let status = call(Failing).unwrap_err();

    assert_eq!(status.code(), Code::Unauthenticated);
}

#[test]
fn a_token_that_is_not_a_valid_header_value_is_internal() {
    let status = call(ForwardedToken::new("line\nbreak")).unwrap_err();

    assert_eq!(status.code(), Code::Internal);
    assert!(!status.message().contains("line"), "{}", status.message());
}
