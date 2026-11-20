//! Tests for the macro argument parser.

use super::*;

use quote::quote;

fn error_message(tokens: TokenStream2) -> String {
    parse_server_arg(tokens)
        .expect_err("arguments must be rejected")
        .to_string()
}

#[test]
fn removed_arguments_are_unknown() {
    for removed in [quote! { lenient }, quote! { claims = my::Claims }] {
        let message = error_message(removed);

        assert!(message.contains("unknown argument"), "{message}");
        assert!(message.contains("server = <path>"), "{message}");
    }
}

#[test]
fn duplicate_server_is_rejected() {
    assert!(error_message(quote! { server = A, server = B }).contains("duplicate `server`"));
}
