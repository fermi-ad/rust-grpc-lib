//! Rejects a method that `#[async_trait]` already rewrote, i.e. the attribute
//! sits below `#[async_trait]` (`async_trait_order_without_policy`).
//!
//! Outer attributes expand first. Once `#[async_trait]` has turned an
//! `async fn` into a `fn` returning a boxed future, the prologue's `?` no
//! longer sits in a `Result`-returning body and the failure would surface as
//! opaque type errors. Detection is structural: no `async` plus an
//! `'async_trait` lifetime parameter. Requiring both avoids flagging a
//! hand-written method that returns a future.

use syn::{Error, Signature};

#[cfg(test)]
mod tests;

/// Errors, spanned on the method name, if `sig` is `#[async_trait]` output.
pub(super) fn check_async_trait_order(sig: &Signature) -> syn::Result<()> {
    let rewritten = sig.asyncness.is_none()
        && sig
            .generics
            .lifetimes()
            .any(|param| param.lifetime.ident == "async_trait");
    if rewritten {
        return Err(Error::new_spanned(
            &sig.ident,
            "`#[access_controlled_service]` must be placed above `#[async_trait]`; \
             `#[async_trait]` has already rewritten this method",
        ));
    }
    Ok(())
}
