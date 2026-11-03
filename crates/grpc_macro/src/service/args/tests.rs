//! Tests for the macro argument parser.

use super::*;

use proc_macro2::TokenStream;
use quote::quote;

fn parse(tokens: TokenStream) -> syn::Result<MacroArgs> {
    syn::parse2(tokens)
}

fn error_message(tokens: TokenStream) -> String {
    parse(tokens)
        .err()
        .expect("arguments must be rejected")
        .to_string()
}

#[test]
fn no_arguments_is_lenient_with_default_claims() {
    let args = parse(quote! {}).unwrap();

    assert!(args.claims.is_none());
    assert!(!args.strict);
}

#[test]
fn strict_is_recognized() {
    let args = parse(quote! { strict }).unwrap();

    assert!(args.strict);
    assert!(args.claims.is_none());
}

#[test]
fn claims_and_strict_combine_in_either_order() {
    for tokens in [
        quote! { strict, claims = my::Claims },
        quote! { claims = my::Claims, strict },
        quote! { claims = my::Claims, strict, },
    ] {
        let args = parse(tokens).unwrap();

        assert!(args.strict);
        assert!(args.claims.is_some());
    }
}

#[test]
fn unknown_argument_lists_the_accepted_ones() {
    let message = error_message(quote! { loose });

    assert!(message.contains("unknown argument `loose`"), "{message}");
    assert!(message.contains("strict"), "{message}");
    assert!(message.contains("claims"), "{message}");
}

#[test]
fn duplicate_arguments_are_rejected() {
    assert!(error_message(quote! { strict, strict }).contains("duplicate `strict`"));
    assert!(error_message(quote! { claims = A, claims = B }).contains("duplicate `claims`"));
}
