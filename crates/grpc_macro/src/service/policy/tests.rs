//! Tests for policy attribute extraction.

use super::*;

use syn::parse_quote;

fn extract(mut list: Vec<Attribute>) -> (syn::Result<Option<Policy>>, Vec<Attribute>) {
    let result = extract_policy(&mut list);
    (result, list)
}

#[test]
fn no_policy_attribute_yields_none_and_keeps_the_others() {
    let (result, remaining) = extract(vec![
        parse_quote!(#[inline]),
        parse_quote!(#[allow(unused)]),
    ]);

    assert!(result.unwrap().is_none());
    assert_eq!(remaining.len(), 2);
}

#[test]
fn roles_is_parsed_and_stripped() {
    let (result, remaining) = extract(vec![
        parse_quote!(#[inline]),
        parse_quote!(#[roles(any("admin"))]),
    ]);

    let Some(Policy::Roles(spec)) = result.unwrap() else {
        panic!("a roles policy was declared");
    };
    assert_eq!(spec.constructor, "any_of");
    assert_eq!(remaining.len(), 1);
}

#[test]
fn any_authenticated_is_parsed_and_stripped() {
    let (result, remaining) = extract(vec![parse_quote!(#[any_authenticated])]);

    assert!(matches!(result.unwrap(), Some(Policy::AnyAuthenticated)));
    assert_eq!(remaining.len(), 0);
}

#[test]
fn public_is_parsed_and_stripped() {
    let (result, remaining) = extract(vec![parse_quote!(#[public])]);

    let policy = result.unwrap().expect("a policy was declared");
    assert!(policy.is_public());
    assert_eq!(remaining.len(), 0);
}

#[test]
fn each_policy_renders_its_access_value() {
    let render = |attr: Attribute| {
        extract(vec![attr])
            .0
            .unwrap()
            .unwrap()
            .access()
            .to_string()
            .replace(' ', "")
    };

    assert_eq!(
        render(parse_quote!(#[public])),
        "::rust_grpc_lib::auth::Access::Public"
    );
    assert_eq!(
        render(parse_quote!(#[any_authenticated])),
        "::rust_grpc_lib::auth::Access::Authenticated"
    );
    assert_eq!(
        render(parse_quote!(#[roles(any("a", roles::B))])),
        "::rust_grpc_lib::auth::Access::any_of(&[\"a\",roles::B])"
    );
    assert_eq!(
        render(parse_quote!(#[roles(all("a"))])),
        "::rust_grpc_lib::auth::Access::all_of(&[\"a\"])"
    );
}

#[test]
fn a_path_qualified_attribute_is_recognized() {
    let (result, _) = extract(vec![parse_quote!(#[rust_grpc_lib::any_authenticated])]);

    assert!(matches!(result.unwrap(), Some(Policy::AnyAuthenticated)));
}

#[test]
fn two_policies_are_rejected() {
    for pair in [
        vec![
            parse_quote!(#[roles(any("a"))]),
            parse_quote!(#[any_authenticated]),
        ],
        vec![
            parse_quote!(#[any_authenticated]),
            parse_quote!(#[roles(all("b"))]),
        ],
        vec![
            parse_quote!(#[roles(any("a"))]),
            parse_quote!(#[roles(all("b"))]),
        ],
        vec![parse_quote!(#[public]), parse_quote!(#[any_authenticated])],
        vec![parse_quote!(#[roles(any("a"))]), parse_quote!(#[public])],
    ] {
        let (result, _) = extract(pair);

        let message = result
            .err()
            .expect("two policies must be rejected")
            .to_string();
        assert!(message.contains("exactly one access policy"), "{message}");
    }
}

#[test]
fn marker_policies_reject_arguments() {
    for attr in [
        parse_quote!(#[any_authenticated(all)]),
        parse_quote!(#[any_authenticated = "x"]),
        parse_quote!(#[public(all)]),
        parse_quote!(#[public = "x"]),
    ] {
        let (result, _) = extract(vec![attr]);

        let message = result
            .err()
            .expect("arguments must be rejected")
            .to_string();
        assert!(message.contains("takes no arguments"), "{message}");
    }
}

#[test]
fn roles_without_a_list_is_rejected() {
    let (result, _) = extract(vec![parse_quote!(#[roles])]);

    assert!(result.is_err());
}
