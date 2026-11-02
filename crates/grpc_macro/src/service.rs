//! Expansion of `#[keycloak_authenticated_service]`.
//!
//! [`expand`] parses the attribute arguments and the `impl` block, builds the
//! per-block [`Env`], then runs every method through the same steps:
//!
//! 1. `async_trait_order` rejects methods that `#[async_trait]` already rewrote.
//! 2. `roles` strips and parses the method's `#[roles(..)]` attribute.
//! 3. `request_param` rewrites a `RequestContext<T>` parameter back to
//!    `tonic::Request<T>` and produces the prologue that rebuilds the context.
//! 4. `guard` emits the role check (and its `tracing` events), which is
//!    prepended to the method body.
//!
//! Errors from different methods are combined so the user sees all of them in
//! one compile.

use std::mem::take;

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{Block, ImplItem, ImplItemFn, ItemImpl};

use args::MacroArgs;
use async_trait_order::check_async_trait_order;
use env::Env;
use guard::build_guard;
use request_param::rewrite_first_param;
use roles::extract_roles_attr;

mod args;
mod async_trait_order;
mod env;
mod guard;
mod request_param;
mod roles;

/// Expands `#[keycloak_authenticated_service(<attr>)]` applied to `<item>`.
pub(crate) fn expand(attr: TokenStream2, item: TokenStream2) -> syn::Result<TokenStream2> {
    let args: MacroArgs = syn::parse2(attr)?;
    let mut impl_block: ItemImpl = syn::parse2(item)?;
    let env = Env::new(args, &impl_block);

    expand_methods(&mut impl_block, &env)?;
    Ok(quote! { #impl_block })
}

/// Processes every method of the block, collecting one error per bad method.
fn expand_methods(impl_block: &mut ItemImpl, env: &Env) -> syn::Result<()> {
    let errors = impl_block
        .items
        .iter_mut()
        .filter_map(|item| match item {
            ImplItem::Fn(method) => Some(method),
            _ => None,
        })
        .filter_map(|method| process_method(method, env).err())
        .reduce(|mut all, err| {
            all.combine(err);
            all
        });
    errors.map_or(Ok(()), Err)
}

/// Rewrites one method: context prologue and role guard are prepended to the
/// original body. A method that needs neither is left untouched.
fn process_method(method: &mut ImplItemFn, env: &Env) -> syn::Result<()> {
    check_async_trait_order(&method.sig)?;

    let roles = extract_roles_attr(&mut method.attrs)?;
    let Some((param, prologue)) = rewrite_first_param(&mut method.sig, roles.is_some())? else {
        // Neither a context parameter nor a role guard: leave the method untouched.
        return Ok(());
    };

    let guard = roles
        .as_ref()
        .map(|spec| build_guard(env, &param, spec, &method.sig.ident))
        .unwrap_or_default();

    let prelude: Block = syn::parse2(quote! {{ #prologue #guard }})?;
    let original_stmts = take(&mut method.block.stmts);
    method.block.stmts = prelude.stmts.into_iter().chain(original_stmts).collect();
    Ok(())
}
