//! Tests for the guard module.

use super::*;
use crate::service::args::MacroArgs;

fn guard_tokens(param: RequestParam) -> String {
    let args: MacroArgs = syn::parse2(quote! {}).unwrap();
    let impl_block: syn::ItemImpl = syn::parse_quote! { impl Echo for Server {} };
    let env = Env::new(args, &impl_block);
    let spec = RolesSpec::Any(vec!["admin".to_string()]);
    let method = Ident::new("echo", Span::call_site());

    build_guard(&env, &param, &spec, &method).to_string()
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
        let tokens = guard_tokens(param);

        assert!(tokens.contains("permission_denied"), "{tokens}");
        assert!(tokens.contains("required role not present"), "{tokens}");
    }
}

#[test]
fn denial_is_logged_as_a_warning_on_the_authz_target() {
    for param in [request_param(), context_param()] {
        let tokens = guard_tokens(param);

        assert!(tokens.contains("tracing"), "{tokens}");
        assert!(tokens.contains("warn"), "{tokens}");
        assert!(tokens.contains("rust_grpc_lib::authz"), "{tokens}");
        assert!(tokens.contains("authorization denied"), "{tokens}");
    }
}

#[test]
fn denial_event_names_the_service_and_method() {
    let tokens = guard_tokens(request_param());

    assert!(tokens.contains("\"Echo\""), "{tokens}");
    assert!(tokens.contains("\"echo\""), "{tokens}");
}

#[test]
fn request_guard_fails_closed_and_logs_on_missing_claims() {
    let tokens = guard_tokens(request_param());

    assert!(
        tokens.contains("authentication is not configured"),
        "{tokens}"
    );
    assert!(tokens.contains("claims_type"), "{tokens}");
}

#[test]
fn context_guard_has_no_missing_claims_path() {
    let tokens = guard_tokens(context_param());

    assert!(!tokens.contains("claims_type"), "{tokens}");
}
