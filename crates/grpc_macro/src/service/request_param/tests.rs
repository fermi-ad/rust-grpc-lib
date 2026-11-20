//! Tests for the request-parameter rewrite.

use super::*;

use quote::ToTokens;
use syn::ItemFn;

fn signature(item: ItemFn) -> Signature {
    item.sig
}

/// The prologue is rendered to a string, which is what the tests inspect.
fn rewrite(mut sig: Signature, public: bool) -> (syn::Result<String>, Signature) {
    let result =
        rewrite_context_param(&mut sig, public).map(|stmt| stmt.to_token_stream().to_string());
    (result, sig)
}

fn error_message(sig: Signature, public: bool) -> String {
    rewrite(sig, public)
        .0
        .expect_err("the parameter must be rejected")
        .to_string()
}

#[test]
fn an_authenticated_context_is_rewritten_to_a_tonic_request() {
    let sig = signature(parse_quote! {
        async fn echo(&self, ctx: AuthenticatedContext<String>) {}
    });

    let (prologue, sig) = rewrite(sig, false);

    let sig = sig.to_token_stream().to_string();
    assert!(
        sig.contains(":: rust_grpc_lib :: __private :: tonic :: Request < String >"),
        "{sig}"
    );
    assert!(!sig.contains("AuthenticatedContext"), "{sig}");
    let prologue = prologue.unwrap();
    assert!(
        prologue.contains(":: rust_grpc_lib :: auth :: AuthenticatedContext :: try_from_request"),
        "{prologue}"
    );
    assert!(
        prologue.contains("let ctx : AuthenticatedContext < String >"),
        "{prologue}"
    );
}

#[test]
fn a_public_context_is_rewritten_for_a_public_method() {
    let sig = signature(parse_quote! {
        async fn ping(&self, ctx: rust_grpc_lib::auth::PublicContext<Ping>) {}
    });

    let (prologue, sig) = rewrite(sig, true);

    assert!(
        sig.to_token_stream()
            .to_string()
            .contains(":: rust_grpc_lib :: __private :: tonic :: Request < Ping >")
    );
    assert!(
        prologue
            .unwrap()
            .contains(":: rust_grpc_lib :: auth :: PublicContext :: try_from_request")
    );
}

#[test]
fn a_mutable_binding_is_kept() {
    let sig = signature(parse_quote! {
        async fn echo(&self, mut ctx: AuthenticatedContext<String>) {}
    });

    let prologue = rewrite(sig, false).0.unwrap();

    assert!(prologue.contains("let mut ctx"), "{prologue}");
}

#[test]
fn a_public_context_on_an_authenticated_method_is_rejected() {
    let sig = signature(parse_quote! {
        async fn echo(&self, ctx: PublicContext<Ping>) {}
    });

    let message = error_message(sig, false);

    assert!(
        message.contains("takes `AuthenticatedContext<T>`, not `PublicContext<T>`"),
        "{message}"
    );
}

#[test]
fn a_method_without_a_request_parameter_is_rejected() {
    let sig = signature(parse_quote! {
        async fn echo(&self) {}
    });

    let message = error_message(sig, false);

    assert!(
        message.contains("takes a request parameter of type `AuthenticatedContext<T>`"),
        "{message}"
    );
}
