//! Detection of `#[async_trait]` placed above `#[keycloak_authenticated_service]`.
//!
//! The macro must run first so it sees `async fn` signatures. If
//! `#[async_trait]` ran first, the body is already wrapped in
//! `Box::pin(async move { .. })` and the guard would fail to type-check with
//! opaque errors. Catching it here gives a precise message instead.

use syn::Signature;

/// Rejects a method that `#[async_trait]` has already rewritten.
///
/// `async_trait` removes `async` and adds an `'async_trait` lifetime parameter;
/// seeing both means the attributes are in the wrong order.
pub(super) fn check_async_trait_order(sig: &Signature) -> syn::Result<()> {
    let rewritten = sig.asyncness.is_none()
        && sig
            .generics
            .lifetimes()
            .any(|param| param.lifetime.ident == "async_trait");
    if rewritten {
        return Err(syn::Error::new_spanned(
            &sig.ident,
            "`#[keycloak_authenticated_service]` must be placed above `#[async_trait]`; \
             `#[async_trait]` has already rewritten this method",
        ));
    }
    Ok(())
}
