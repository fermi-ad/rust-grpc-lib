//! The per-`impl`-block context shared by every method expansion.
//!
//! [`Env`] holds what the emitted code needs to know about the block as a
//! whole: which claims type to read, the service name to report in denial
//! logs, and whether the block is `strict`.
//!
//! The emitted code is rooted at `::rust_grpc_lib`. By policy every consumer
//! depends on `rust-grpc-lib` (and `tonic` and `tracing`) under their normal
//! names, so no crate-name resolution is attempted.

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::ItemImpl;

use super::args::MacroArgs;

/// Everything the per-method expansion needs to know about the `impl` block.
pub(super) struct Env {
    /// The claims type read by guards on `Request<T>` methods.
    pub(super) claims_ty: TokenStream2,
    /// Name reported as `rpc.service` in denial events.
    pub(super) service: String,
    /// Whether every method must declare an access policy.
    pub(super) strict: bool,
}

impl Env {
    /// Builds the context from the attribute arguments and the `impl` block.
    ///
    /// The claims type defaults to `KeycloakClaims`. The service name is the
    /// last segment of the implemented trait, or the self type for an
    /// inherent `impl`.
    pub(super) fn new(args: MacroArgs, impl_block: &ItemImpl) -> Self {
        let strict = args.strict;
        let claims_ty = if let Some(path) = args.claims {
            quote! { #path }
        } else {
            quote! { ::rust_grpc_lib::auth::KeycloakClaims }
        };
        let service = service_name(impl_block);

        Self {
            claims_ty,
            service,
            strict,
        }
    }
}

/// The name reported as `rpc.service`: the implemented trait's last path
/// segment, or the self type's tokens when the block has no trait.
fn service_name(impl_block: &ItemImpl) -> String {
    if let Some((_, path, _)) = &impl_block.trait_ {
        path.segments
            .last()
            .map(|seg| seg.ident.to_string())
            .unwrap_or_default()
    } else {
        let self_ty = &impl_block.self_ty;
        quote! { #self_ty }.to_string()
    }
}
