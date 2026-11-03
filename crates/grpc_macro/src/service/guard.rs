//! The access guards emitted at the top of a method.
//!
//! [`build`] picks the guard for a method's declared policy: the role check of
//! `#[roles(..)]` or the claims check of `#[any_authenticated]`. Both read the
//! claims (from the rewritten context, or from the request extensions) and fail
//! closed. The role guard also returns `permission_denied` when the check
//! fails.
//!
//! They log through `tracing`, unconditionally: an `error` when the claims
//! extension is missing (a server misconfiguration) and, for the role guard, a
//! `warn` on the `rust_grpc_lib::authz` target when a caller is denied. Neither
//! event ever carries the token, the other claims or the roles the caller
//! holds, only the `sub` claim and the roles the endpoint required.
//!
//! The emitted code names `::tonic` and `::tracing`, which resolve to whatever
//! versions the consumer depends on, so every consumer of the macro needs
//! `tracing` as a direct dependency.

use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{format_ident, quote};
use syn::Ident;

use super::env::Env;
use super::policy::Policy;
use super::request_param::RequestParam;
use super::roles::RolesSpec;

#[cfg(test)]
mod tests;

/// Builds the guard for a method's declared policy, or nothing if it has none.
pub(super) fn build(
    env: &Env,
    param: &RequestParam,
    policy: Option<&Policy>,
    method: &Ident,
) -> TokenStream2 {
    match policy {
        Some(Policy::Roles(spec)) => build_guard(env, param, spec, method),
        Some(Policy::AnyAuthenticated) => build_authenticated_guard(env, param),
        None => TokenStream2::new(),
    }
}

/// Builds the role-check guard block as a [`TokenStream2`].
fn build_guard(env: &Env, param: &RequestParam, spec: &RolesSpec, method: &Ident) -> TokenStream2 {
    let service = &env.service;
    let method = method.to_string();
    let roles = spec.roles();
    let mode = spec.mode();
    let check = format_ident!("{mode}");

    let claims = Ident::new("__claims", Span::mixed_site());
    let required = Ident::new("__required", Span::mixed_site());
    let remote = Ident::new("__remote", Span::mixed_site());

    let claims_expr = claims_expr(env, param);
    let remote_addr = match param {
        RequestParam::Request(request) => quote! { #request.remote_addr() },
        RequestParam::Context(ctx) => quote! { #ctx.remote_addr },
    };

    quote! {
        {
            let #claims = #claims_expr;
            let #required: &[&str] = &[#(#roles),*];
            if !#required.iter().#check(|r| #claims.has_role(r)) {
                let #remote = #remote_addr.map(|addr| addr.to_string());
                ::tracing::warn!(
                    target: "rust_grpc_lib::authz",
                    user = ::rust_grpc_lib::auth::Claims::sub(#claims),
                    rpc.service = #service,
                    rpc.method = #method,
                    required_roles = ?#required,
                    role_mode = #mode,
                    remote_addr = #remote.as_deref(),
                    "authorization denied"
                );
                return ::core::result::Result::Err(
                    ::tonic::Status::permission_denied("required role not present")
                );
            }
        }
    }
}

/// Builds the guard for `#[any_authenticated]`: the claims must be present,
/// and no role is checked.
///
/// A `RequestContext` parameter already proved that when it was built, so only
/// a `Request<T>` parameter needs a check; the result is empty for a context.
fn build_authenticated_guard(env: &Env, param: &RequestParam) -> TokenStream2 {
    match param {
        RequestParam::Context(_) => TokenStream2::new(),
        RequestParam::Request(_) => {
            let claims_expr = claims_expr(env, param);
            quote! {
                {
                    let _ = #claims_expr;
                }
            }
        }
    }
}

/// How to reach the claims for this parameter form. A `Request<T>` parameter
/// reads the extension and fails closed when it is missing; a context already
/// holds the claims.
fn claims_expr(env: &Env, param: &RequestParam) -> TokenStream2 {
    let claims_ty = &env.claims_ty;
    match param {
        RequestParam::Request(request) => quote! {
            match #request.extensions().get::<#claims_ty>() {
                ::core::option::Option::Some(claims) => claims,
                ::core::option::Option::None => {
                    ::tracing::error!(
                        claims_type = ::core::any::type_name::<#claims_ty>(),
                        "request has no claims extension; the JWT validation layer or \
                         interceptor is not installed for this service"
                    );
                    return ::core::result::Result::Err(
                        ::tonic::Status::internal("authentication is not configured")
                    );
                }
            }
        },
        RequestParam::Context(ctx) => quote! { &#ctx.claims },
    }
}
