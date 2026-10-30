//! Handling of a handler's request parameter.
//!
//! The first non-`self` parameter of a method is either a plain
//! `tonic::Request<T>` or a `RequestContext<T>`. A context parameter is
//! rewritten in the emitted signature back to `tonic::Request<T>`, so the
//! method still matches the tonic-generated trait, and a prologue rebuilds the
//! context under the user's own binding.

use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::quote;
use syn::{FnArg, GenericArgument, Ident, Pat, PathArguments, Signature, Type, parse_quote};

/// How the first non-`self` parameter of a method is consumed by the guard.
pub(super) enum RequestParam {
    /// A `tonic::Request<T>` parameter with the given name.
    Request(Ident),
    /// A `RequestContext<T>` parameter with the given name, which was
    /// rewritten to a `Request<T>`.
    Context(Ident),
}

/// Inspects the first non-`self` parameter.
///
/// - A `RequestContext<T>` parameter is rewritten in `sig` to
///   `tonic::Request<T>` under a hygienic name, and the returned prologue
///   rebuilds the context from it.
/// - A `Request<T>` parameter is left alone (with an empty prologue); it is
///   only inspected when `needs_guard` is set, to find the name the guard reads
///   extensions from.
///
/// Returns `None` when the method needs neither.
pub(super) fn rewrite_first_param(
    sig: &mut Signature,
    needs_guard: bool,
) -> syn::Result<Option<(RequestParam, TokenStream2)>> {
    let Some(pat_type) = sig.inputs.iter_mut().find_map(|arg| match arg {
        FnArg::Typed(pat_type) => Some(pat_type),
        FnArg::Receiver(_) => None,
    }) else {
        return if needs_guard {
            Err(syn::Error::new_spanned(
                &sig.ident,
                "`#[roles]` requires the method to take a request parameter",
            ))
        } else {
            Ok(None)
        };
    };

    let Some(message_ty) = request_context_message_type(&pat_type.ty)? else {
        if !needs_guard {
            return Ok(None);
        }
        let Pat::Ident(pat_ident) = pat_type.pat.as_ref() else {
            return Err(syn::Error::new_spanned(
                &pat_type.pat,
                "`#[roles]` needs a named request parameter, such as `request: Request<T>`",
            ));
        };
        let param = RequestParam::Request(pat_ident.ident.clone());
        return Ok(Some((param, TokenStream2::new())));
    };

    // A `RequestContext<T>` parameter: keep the user's binding for the prologue.
    let user_pat = match pat_type.pat.as_ref() {
        Pat::Ident(pat_ident) if pat_ident.subpat.is_none() && pat_ident.by_ref.is_none() => {
            pat_ident.clone()
        }
        other => {
            return Err(syn::Error::new_spanned(
                other,
                "a `RequestContext` parameter must be a plain binding, such as `ctx` or `mut ctx`",
            ));
        }
    };
    let user_ty = (*pat_type.ty).clone();

    let hygienic = Ident::new("__rust_grpc_lib_request", Span::mixed_site());
    *pat_type.pat = parse_quote! { #hygienic };
    *pat_type.ty = parse_quote! { ::tonic::Request<#message_ty> };

    let prologue = quote! {
        let #user_pat: #user_ty =
            ::rust_grpc_lib::auth::RequestContext::try_from_request(#hygienic)?;
    };
    Ok(Some((RequestParam::Context(user_pat.ident), prologue)))
}

/// If `ty` is a path whose last segment is `RequestContext`, returns its first
/// type argument (the message type).
fn request_context_message_type(ty: &Type) -> syn::Result<Option<Type>> {
    let Type::Path(type_path) = ty else {
        return Ok(None);
    };
    if type_path.qself.is_some() {
        return Ok(None);
    }
    let Some(segment) = type_path.path.segments.last() else {
        return Ok(None);
    };
    if segment.ident != "RequestContext" {
        return Ok(None);
    }
    if let PathArguments::AngleBracketed(args) = &segment.arguments
        && let Some(message_ty) = args.args.iter().find_map(|arg| match arg {
            GenericArgument::Type(message_ty) => Some(message_ty.clone()),
            _ => None,
        })
    {
        return Ok(Some(message_ty));
    }
    Err(syn::Error::new_spanned(
        segment,
        "`RequestContext` needs a message type argument, such as `RequestContext<MyRequest>`",
    ))
}
