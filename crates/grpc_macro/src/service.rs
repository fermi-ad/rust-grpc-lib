//! Expansion of `#[keycloak_authenticated_service]`.
//!
//! [`expand`] parses the attribute arguments and the `impl` block, builds the
//! per-block [`Env`], then runs every method through the same steps:
//!
//! 1. `async_trait_order` rejects methods that `#[async_trait]` already rewrote.
//!    It runs first and unconditionally, so a wrong order is reported as such
//!    rather than as a missing policy or an opaque type error.
//! 2. `policy` strips and parses the method's access policy (`#[roles(..)]` or
//!    `#[any_authenticated]`). In `strict` mode a method with no policy is an
//!    error.
//! 3. `request_param` rewrites a `RequestContext<T>` parameter back to
//!    `tonic::Request<T>` and produces the prologue that rebuilds the context.
//! 4. `guard` emits the check for the policy (and its `tracing` events), which
//!    is prepended to the method body.
//!
//! Errors from different methods are combined so the user sees all of them in
//! one compile.

use std::mem::take;

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{Block, Error, ImplItem, ImplItemFn, ItemImpl};

use args::MacroArgs;
use async_trait_order::check_async_trait_order;
use env::Env;
use policy::extract_policy;
use request_param::rewrite_first_param;

mod args;
mod async_trait_order;
mod env;
mod guard;
mod policy;
mod request_param;
mod roles;

/// The policy attributes, for the entry points in the crate root that report
/// one nobody stripped.
pub(crate) use policy::Kind;

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

/// Rewrites one method: context prologue and access guard are prepended to the
/// original body. A method that needs neither is left untouched.
fn process_method(method: &mut ImplItemFn, env: &Env) -> syn::Result<()> {
    check_async_trait_order(&method.sig)?;

    let policy = extract_policy(&mut method.attrs)?;
    if env.strict && policy.is_none() {
        return Err(missing_policy(method));
    }

    let Some((param, prologue)) = rewrite_first_param(&mut method.sig, policy.is_some())? else {
        // Neither a context parameter nor a claims check: leave the method untouched.
        return Ok(());
    };

    let guard = guard::build(env, &param, policy.as_ref(), &method.sig.ident);

    let prelude: Block = syn::parse2(quote! {{ #prologue #guard }})?;
    let original_stmts = take(&mut method.block.stmts);
    method.block.stmts = prelude.stmts.into_iter().chain(original_stmts).collect();
    Ok(())
}

/// The `strict` error for a method that declares no access policy.
fn missing_policy(method: &ImplItemFn) -> Error {
    let name = &method.sig.ident;
    Error::new_spanned(
        name,
        format!(
            "method `{name}` declares no access policy, which `strict` requires; add {}",
            Kind::alternatives()
        ),
    )
}
