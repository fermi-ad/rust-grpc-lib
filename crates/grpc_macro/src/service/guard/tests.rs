//! Tests for the guard module.

use super::*;
use crate::service::args::MacroArgs;
use crate::service::roles::RoleItem;

fn env() -> Env {
    let args: MacroArgs = syn::parse2(quote! {}).unwrap();
    let impl_block: syn::ItemImpl = syn::parse_quote! { impl Echo for Server {} };
    Env::new(args, &impl_block)
}

fn guard_tokens(param: &RequestParam) -> String {
    let spec = RolesSpec::Any(vec![RoleItem::Literal(syn::parse_quote!("admin"))]);
    let method = Ident::new("echo", Span::call_site());

    build_guard(&env(), param, &spec, &method).to_string()
}

fn request_param() -> RequestParam {
    RequestParam::Request(Ident::new("request", Span::call_site()))
}

fn context_param() -> RequestParam {
    RequestParam::Context(Ident::new("ctx", Span::call_site()))
}

#[test]
fn guard_always_rejects_with_permission_denied() {
    for param in [request_param(), context_param()] {
        let tokens = guard_tokens(&param);

        assert!(tokens.contains("permission_denied"), "{tokens}");
        assert!(tokens.contains("required role not present"), "{tokens}");
    }
}

#[test]
fn denial_is_logged_as_a_warning_on_the_authz_target() {
    for param in [request_param(), context_param()] {
        let tokens = guard_tokens(&param);

        assert!(tokens.contains("tracing"), "{tokens}");
        assert!(tokens.contains("warn"), "{tokens}");
        assert!(tokens.contains("rust_grpc_lib::authz"), "{tokens}");
        assert!(tokens.contains("authorization denied"), "{tokens}");
    }
}

#[test]
fn denial_event_names_the_service_and_method() {
    let tokens = guard_tokens(&request_param());

    assert!(tokens.contains("\"Echo\""), "{tokens}");
    assert!(tokens.contains("\"echo\""), "{tokens}");
}

#[test]
fn request_guard_fails_closed_and_logs_on_missing_claims() {
    let tokens = guard_tokens(&request_param());

    assert!(
        tokens.contains("authentication is not configured"),
        "{tokens}"
    );
    assert!(tokens.contains("claims_type"), "{tokens}");
}

#[test]
fn context_guard_has_no_missing_claims_path() {
    let tokens = guard_tokens(&context_param());

    assert!(!tokens.contains("claims_type"), "{tokens}");
}

#[test]
fn typed_roles_are_emitted_as_paths() {
    let spec = RolesSpec::All(vec![
        RoleItem::Path(syn::parse_quote!(roles::OPERATOR)),
        RoleItem::Literal(syn::parse_quote!("admin")),
    ]);
    let method = Ident::new("echo", Span::call_site());

    let tokens = build_guard(&env(), &request_param(), &spec, &method).to_string();

    assert!(tokens.contains("roles :: OPERATOR"), "{tokens}");
    assert!(tokens.contains("\"admin\""), "{tokens}");
}

#[test]
fn authenticated_guard_on_a_request_fails_closed_without_a_role_check() {
    let tokens = build_authenticated_guard(&env(), &request_param()).to_string();

    assert!(
        tokens.contains("authentication is not configured"),
        "{tokens}"
    );
    assert!(!tokens.contains("permission_denied"), "{tokens}");
    assert!(!tokens.contains("has_role"), "{tokens}");
}

#[test]
fn authenticated_guard_on_a_context_is_empty() {
    assert!(build_authenticated_guard(&env(), &context_param()).is_empty());
}
