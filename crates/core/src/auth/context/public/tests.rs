//! Tests for [`super::PublicContext`].

use super::*;

use tonic::Code;

use crate::auth::context::tests::{method, public};

#[test]
fn a_public_context_has_the_method_and_the_message_but_no_claims() {
    let ctx = PublicContext::try_from_request(public("hello".to_string())).unwrap();

    assert_eq!(ctx.message, "hello");
    assert_eq!(ctx.rpc, method());
    assert!(ctx.remote_addr.is_none());
}

#[test]
fn a_public_context_without_a_method_fails_closed() {
    let err = PublicContext::try_from_request(Request::new(())).expect_err("no RpcMethod");

    assert_eq!(err.code(), Code::Internal);
    assert_eq!(err.message(), "authentication is not configured");
}
