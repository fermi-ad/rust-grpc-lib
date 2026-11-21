//! Rewrites the first non-`self` parameter, `RequestContext<T>` (`#[roles]`,
//! `#[any_authenticated]`) or `PublicContext<T>` (`#[public]`), to the
//! `::rust_grpc_lib::__private::tonic::Request<T>` the tonic trait expects, and
//! returns the prologue:
//!
//! ```text
//! let ctx: RequestContext<T> =
//!     ::rust_grpc_lib::auth::RequestContext::try_from_request(__rust_grpc_lib_request)?;
//! ```
//!
//! Security:
//! - `try_from_request` fails closed (`Internal`) unless the call came
//!   through `add_service`: it needs the `RpcMethod` extension and, for
//!   `RequestContext`, the verified caller the gate inserts.
//! - The request is bound under a `Span::mixed_site()` name, so the body
//!   cannot reach the raw request.
//! - The user's type annotation is kept and must unify with the real context,
//!   so a lookalike type does not compile.
//! - The context must match the policy, so a `#[public]` method never gets
//!   claims (`request_context_on_public`).
//!
//! A plain `tonic::Request<T>` is rejected (`request_in_annotated_block`) so
//! every handler gets the method and span via its context. Contexts match by
//! last path segment; the other context kind is recognized only for a precise
//! error. Other errors: no parameter, no message type
//! (`context_without_message_type`), a non-binding pattern
//! (`context_destructured`).

use proc_macro2::Span;
use syn::{
    Error, FnArg, GenericArgument, Ident, Pat, PathArguments, Signature, Stmt, Type, parse_quote,
};

#[cfg(test)]
mod tests;

const REQUEST: &str = "RequestContext";
const PUBLIC: &str = "PublicContext";

/// See the module docs. `public` selects `PublicContext` over
/// `RequestContext`.
pub(super) fn rewrite_context_param(sig: &mut Signature, public: bool) -> syn::Result<Stmt> {
    let (expected, other) = if public {
        (PUBLIC, REQUEST)
    } else {
        (REQUEST, PUBLIC)
    };

    let Some(pat_type) = sig.inputs.iter_mut().find_map(|arg| match arg {
        FnArg::Typed(pat_type) => Some(pat_type),
        FnArg::Receiver(_) => None,
    }) else {
        return Err(Error::new_spanned(
            &sig.ident,
            format!("a method of this block takes a `{expected}<T>` request parameter"),
        ));
    };

    let message_ty = context_message_type(&pat_type.ty, expected, other)?;

    let user_pat = match pat_type.pat.as_ref() {
        Pat::Ident(pat_ident) if pat_ident.subpat.is_none() && pat_ident.by_ref.is_none() => {
            pat_ident.clone()
        }
        other => {
            return Err(Error::new_spanned(
                other,
                format!(
                    "a `{expected}` parameter must be a plain binding, such as `ctx` or `mut ctx`"
                ),
            ));
        }
    };
    let user_ty = (*pat_type.ty).clone();

    let hygienic = Ident::new("__rust_grpc_lib_request", Span::mixed_site());
    *pat_type.pat = parse_quote! { #hygienic };
    *pat_type.ty = parse_quote! { ::rust_grpc_lib::__private::tonic::Request<#message_ty> };

    let context = Ident::new(expected, Span::call_site());
    Ok(parse_quote! {
        let #user_pat: #user_ty =
            ::rust_grpc_lib::auth::#context::try_from_request(#hygienic)?;
    })
}

/// Returns the first type argument of `ty` if it is the `expected` context.
/// `other` is recognized only to report it as the wrong one.
fn context_message_type(ty: &Type, expected: &str, other: &str) -> syn::Result<Type> {
    let segment = match ty {
        Type::Path(path) if path.qself.is_none() => path.path.segments.last(),
        _ => None,
    };
    let Some(segment) = segment.filter(|s| s.ident == expected || s.ident == other) else {
        return Err(Error::new_spanned(
            ty,
            format!(
                "the request parameter must be `{expected}<T>`, not a plain `tonic::Request<T>`; \
                 the context carries the method and the span, and the claims for authenticated \
                 methods"
            ),
        ));
    };

    let name = &segment.ident;
    let message_ty = match &segment.arguments {
        PathArguments::AngleBracketed(args) => args.args.iter().find_map(|arg| match arg {
            GenericArgument::Type(message_ty) => Some(message_ty),
            _ => None,
        }),
        _ => None,
    };
    let Some(message_ty) = message_ty else {
        return Err(Error::new_spanned(
            segment,
            format!("`{name}` needs a message type argument, such as `{name}<MyRequest>`"),
        ));
    };

    if name != expected {
        let reason = if expected == PUBLIC {
            "a `#[public]` method takes `PublicContext<T>`, not `RequestContext<T>`: nobody is \
             authenticated, so there are no claims to give it"
                .to_owned()
        } else {
            format!(
                "a method with a role or authentication policy takes `RequestContext<T>`, not \
                 `{name}<T>`"
            )
        };
        return Err(Error::new_spanned(ty, reason));
    }
    Ok(message_ty.clone())
}
