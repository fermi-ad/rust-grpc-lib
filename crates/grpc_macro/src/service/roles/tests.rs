//! Tests for the `#[roles(...)]` argument parser.

use super::*;

use quote::quote;

fn parse(tokens: TokenStream2) -> syn::Result<RolesSpec> {
    syn::parse2(tokens)
}

fn error_message(tokens: TokenStream2) -> String {
    parse(tokens)
        .err()
        .expect("roles must be rejected")
        .to_string()
}

fn rendered(spec: &RolesSpec) -> Vec<String> {
    spec.roles.iter().map(TokenStream2::to_string).collect()
}

#[test]
fn any_with_string_literals() {
    let spec = parse(quote! { any("viewer", "operator") }).unwrap();

    assert_eq!(spec.constructor, "any_of");
    assert_eq!(rendered(&spec), ["\"viewer\"", "\"operator\""]);
}

#[test]
fn all_with_string_literals() {
    let spec = parse(quote! { all("operator") }).unwrap();

    assert_eq!(spec.constructor, "all_of");
    assert_eq!(rendered(&spec), ["\"operator\""]);
}

#[test]
fn paths_are_accepted_in_place_of_literals() {
    let spec = parse(quote! { any(roles::OPERATOR, ::auth::roles::ADMIN, ADMIN) }).unwrap();

    assert_eq!(
        rendered(&spec),
        ["roles :: OPERATOR", ":: auth :: roles :: ADMIN", "ADMIN"]
    );
}

#[test]
fn an_expression_that_is_neither_a_literal_nor_a_path_is_rejected() {
    for tokens in [
        quote! { any(42) },
        quote! { any(true) },
        quote! { any(&ADMIN) },
        quote! { any(b"admin") },
    ] {
        let message = error_message(tokens);

        assert!(
            message.contains("expected a string literal or a path"),
            "{message}"
        );
    }
}

#[test]
fn a_path_followed_by_an_operator_is_rejected() {
    assert!(parse(quote! { any(ADMIN + 1) }).is_err());
}

#[test]
fn an_empty_list_is_rejected() {
    let message = error_message(quote! { any() });

    assert!(message.contains("at least one role"), "{message}");
}

#[test]
fn tokens_after_the_list_are_rejected() {
    assert!(parse(quote! { any("a") extra }).is_err());
}
