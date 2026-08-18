//! Procedural macros for `rust-grpc-lib`.
//!
//! This crate exposes four public macros:
//!
//! - [`GrpcClient`] — derive macro that generates
//!   `from_endpoint_with_provider` on a tonic client struct, wiring it into
//!   the process-wide channel pool with JWT auth via `ClientJwtInterceptor`.
//! - [`GrpcNoAuthClient`] — derive macro that generates `from_endpoint` (no
//!   auth) on a tonic client struct. For test harnesses only; requires the
//!   `unauthenticated` feature on `rust-grpc-lib`.
//! - [`grpc_service`] — attribute macro applied to an `impl Trait for Type`
//!   block that injects Keycloak role-checking guards into methods annotated
//!   with `#[roles(...)]`.
//! - [`roles`] — marker attribute consumed by [`grpc_service`]. A pass-through
//!   no-op when used without `#[grpc_service]` on the enclosing `impl` block.
//!
//! Internal helpers (`RolesSpec`, `RolesArgs`, `extract_roles_attr`,
//! `first_param_ident`, `build_guard`) are private to this crate.

use std::{iter, mem::take};

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};

use quote::quote;
use syn::{
    Attribute, DeriveInput, FnArg, Ident, ImplItem, ImplItemFn, ItemImpl, Lit, Meta, MetaList, Pat,
    Path, Token,
    parse::{Parse, ParseStream},
    parse_macro_input,
};

#[proc_macro_derive(GrpcClient)]
pub fn grpc_client_derive(input: TokenStream) -> TokenStream {
    let parsed = parse_macro_input!(input as DeriveInput);
    let name = &parsed.ident;

    // Use `::rust_grpc_lib` so the impl resolves against the `core` crate when this
    // derive is expanded, not against any re-export in a consuming crate.
    let as_grpc_client = quote! {
        impl #name<::tonic::transport::Channel>
        {
            pub fn from_endpoint_with_provider<P: ::rust_grpc_lib::auth::TokenProvider>(
                endpoint: &str,
                provider: P,
            ) -> Result<#name<::tonic::service::interceptor::InterceptedService<::tonic::transport::Channel, ::rust_grpc_lib::auth::ClientJwtInterceptor<P>>>, ::tonic::transport::Error>
            {
                let channel = ::rust_grpc_lib::pool::get_channel(endpoint)?;
                Ok(#name::new(::tonic::service::interceptor::InterceptedService::new(channel, ::rust_grpc_lib::auth::ClientJwtInterceptor { provider })))
            }
        }
    };
    TokenStream::from(as_grpc_client)
}

#[proc_macro_derive(GrpcNoAuthClient)]
pub fn grpc_no_auth_client_derive(input: TokenStream) -> TokenStream {
    let parsed = parse_macro_input!(input as DeriveInput);
    let name = &parsed.ident;

    // Use `::rust_grpc_lib` so the impl resolves against the `core` crate when this
    // derive is expanded, not against any re-export in a consuming crate.
    let as_grpc_client = quote! {
        impl #name<::tonic::transport::Channel>
        {
            pub fn from_endpoint(
                endpoint: &str,
            ) -> Result<Self, ::tonic::transport::Error>
            {
                let channel = ::rust_grpc_lib::pool::get_channel(endpoint)?;
                Ok(#name::new(channel))
            }
        }
    };
    TokenStream::from(as_grpc_client)
}

/// Marker attribute consumed by [`grpc_service`].
///
/// When used standalone (without `#[grpc_service]` on the enclosing `impl`
/// block) this attribute is a pass-through no-op so that the code still
/// compiles. `#[grpc_service]` strips and processes it before emitting the
/// final `impl` block.
///
/// # Usage
///
/// ```rust,ignore
/// #[grpc_service]
/// impl MyService for MyServer {
///     #[roles(any("operator", "admin"))]
///     async fn set_data(&self, request: Request<SetDataRequest>)
///         -> Result<Response<SetDataResponse>, Status>
///     {
///         // ...
///     }
/// }
/// ```
#[proc_macro_attribute]
pub fn roles(_attr: TokenStream, item: TokenStream) -> TokenStream {
    // Pass-through; grpc_service strips and processes this attribute.
    item
}

/// Attribute macro applied to an `impl Trait for Type` block that injects
/// Keycloak role-checking guards into methods annotated with `#[roles(...)]`.
///
/// # Role check variants
///
/// - `#[roles(any("r1", "r2"))]` — at least one of the listed roles must be
///   present in the JWT claims.
/// - `#[roles(all("r1", "r2"))]` — every listed role must be present.
/// - No `#[roles(...)]` attribute — the method is left untouched (a valid JWT
///   is still required by the `JwtValidationLayer`, but no role check is
///   injected by this macro).
///
/// # Generated code shape
///
/// ```rust,ignore
/// async fn set_data(&self, request: Request<SetDataRequest>)
///     -> Result<Response<SetDataResponse>, Status>
/// {
///     {
///         let __claims = request
///             .extensions()
///             .get::<::rust_grpc_lib::auth::KeycloakClaims>()
///             .ok_or_else(|| ::tonic::Status::internal(
///                 "JWT claims not populated; ensure JwtValidationLayer is installed",
///             ))?;
///         if !["operator", "admin"].iter().any(|r| __claims.has_role(r)) {
///             return Err(::tonic::Status::permission_denied("required role not present"));
///         }
///     }
///     // original body …
/// }
/// ```
#[proc_macro_attribute]
pub fn grpc_service(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut impl_block = parse_macro_input!(item as ItemImpl);

    for impl_item in &mut impl_block.items {
        let ImplItem::Fn(method) = impl_item else {
            continue;
        };

        // Find and remove the #[roles(...)] attribute, capturing its content.
        let roles_attr = match extract_roles_attr(&mut method.attrs) {
            Ok(roles_attr) => roles_attr,
            Err(e) => return e,
        };

        let Some(roles_spec) = roles_attr else {
            // No roles attribute — leave method untouched.
            continue;
        };

        // Determine the name of the first non-self parameter so we can call
        // `.extensions()` on it.
        let request_ident = first_param_ident(method);

        // Build the guard block.
        let guard = build_guard(request_ident, &roles_spec);

        // Prepend the guard to the existing method body.
        let original_stmts = take(&mut method.block.stmts);
        match syn::parse2(quote! { #guard }) {
            Ok(guard_stmt) => {
                method.block.stmts = iter::once(guard_stmt).chain(original_stmts).collect()
            }
            Err(err) => return TokenStream::from(err.to_compile_error()),
        }
    }

    TokenStream::from(quote! { #impl_block })
}

// Internal helpers

/// Describes the parsed content of a `#[roles(...)]` attribute.
enum RolesSpec {
    Any(Vec<String>),
    All(Vec<String>),
}

/// Parses `any("r1", "r2")` or `all("r1", "r2")` from a [`MetaList`] token
/// stream.
struct RolesArgs {
    spec: RolesSpec,
}

impl Parse for RolesArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        // Expect an identifier: `any` or `all`.
        let variant: syn::Ident = input.parse()?;
        let variant_str = variant.to_string();

        // Expect parenthesised list of string literals.
        let content;
        syn::parenthesized!(content in input);

        let mut roles = Vec::new();
        loop {
            if content.is_empty() {
                break;
            }
            let Lit::Str(s) = content.parse()? else {
                return Err(syn::Error::new_spanned(
                    &variant,
                    "roles must be string literals",
                ));
            };
            roles.push(s.value());
            if content.is_empty() {
                break;
            }
            let _comma: Token![,] = content.parse()?;
        }

        let spec = match variant_str.as_str() {
            "any" => RolesSpec::Any(roles),
            "all" => RolesSpec::All(roles),
            other => {
                return Err(syn::Error::new_spanned(
                    &variant,
                    format!("expected `any` or `all`, found `{other}`"),
                ));
            }
        };

        Ok(RolesArgs { spec })
    }
}

/// Returns `true` if the [`Path`] is a single-segment path equal to `"roles"`.
fn path_is_roles(path: &Path) -> bool {
    path.get_ident().map(|i| i == "roles").unwrap_or(false)
}

/// Finds the first `#[roles(...)]` attribute in `attrs`, removes it, and
/// returns the parsed [`RolesSpec`]. Returns `None` if no such attribute
/// exists.
fn extract_roles_attr(attrs: &mut Vec<Attribute>) -> Result<Option<RolesSpec>, TokenStream> {
    let Some(tokens) = attrs
        .iter()
        .position(|a| matches!(&a.meta, Meta::List(ml) if path_is_roles(&ml.path)))
        .and_then(|pos| {
            let attr = attrs.remove(pos);
            match attr.meta {
                Meta::List(MetaList { tokens, .. }) => Some(tokens),
                _ => None,
            }
        })
    else {
        return Ok(None);
    };

    syn::parse2::<RolesArgs>(tokens)
        .map(|args| Some(args.spec))
        .map_err(|e| TokenStream::from(e.to_compile_error()))
}

/// Returns the [`Ident`] of the first non-`self` parameter of `method`,
/// or falls back to the identifier `request` if none can be found.
fn first_param_ident(method: &ImplItemFn) -> Ident {
    for input in &method.sig.inputs {
        if let FnArg::Typed(pat_type) = input
            && let Pat::Ident(pat_ident) = pat_type.pat.as_ref()
        {
            return pat_ident.ident.clone();
        }
    }
    // Fallback — should not happen for well-formed gRPC service methods.
    Ident::new("request", Span::call_site())
}

/// Builds the role-check guard block as a [`TokenStream2`].
fn build_guard(request_ident: Ident, spec: &RolesSpec) -> TokenStream2 {
    let (roles, iter_method) = match spec {
        RolesSpec::Any(r) => (r, quote! { any }),
        RolesSpec::All(r) => (r, quote! { all }),
    };

    // Build the array literal: ["r1", "r2", ...]
    let role_literals: Vec<TokenStream2> = roles.iter().map(|r| quote! { #r }).collect();

    quote! {
        {
            let __claims = #request_ident
                .extensions()
                .get::<::rust_grpc_lib::auth::KeycloakClaims>()
                .ok_or_else(|| ::tonic::Status::internal(
                    "JWT claims not populated; ensure JwtValidationLayer is installed"
                ))?;
            if ![#(#role_literals),*].iter().#iter_method(|r| __claims.has_role(r)) {
                return Err(::tonic::Status::permission_denied("required role not present"));
            }
        }
    }
}
