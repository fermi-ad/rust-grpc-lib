//! The role-check guard emitted at the top of a guarded method.
//!
//! The guard reads the claims (from the rewritten context, or from the request
//! extensions), checks the required roles, and returns `permission_denied` if
//! the check fails.
//!
//! It also logs through `tracing`, unconditionally: an `error` when the claims
//! extension is missing (a server misconfiguration) and a `warn` on the
//! `rust_grpc_lib::authz` target when a caller is denied. Neither event ever
//! carries the token, the other claims or the roles the caller holds, only the
//! `sub` claim and the roles the endpoint required.
//!
//! The emitted code names `::tonic` and `::tracing`, which resolve to whatever
//! versions the consumer depends on, so every consumer of the macro needs
//! `tracing` as a direct dependency.

use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{format_ident, quote};
use syn::Ident;

use super::env::Env;
use super::request_param::RequestParam;
use super::roles::RolesSpec;

#[cfg(test)]
mod tests;

/// Builds the role-check guard block as a [`TokenStream2`].
pub(super) fn build_guard(
    env: &Env,
    param: &RequestParam,
    spec: &RolesSpec,
    method: &Ident,
) -> TokenStream2 {
    let claims_ty = &env.claims_ty;
    let service = &env.service;
    let method = method.to_string();
    let roles = spec.roles();
    let mode = spec.mode();
    let check = format_ident!("{mode}");

    let claims = Ident::new("__claims", Span::mixed_site());
    let required = Ident::new("__required", Span::mixed_site());
    let remote = Ident::new("__remote", Span::mixed_site());

    // How to reach the claims and the peer address for this parameter form.
    let (claims_expr, remote_addr) = match param {
        RequestParam::Request(request) => (
            quote! {
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
            quote! { #request.remote_addr() },
        ),
        RequestParam::Context(ctx) => (quote! { &#ctx.claims }, quote! { #ctx.remote_addr }),
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
