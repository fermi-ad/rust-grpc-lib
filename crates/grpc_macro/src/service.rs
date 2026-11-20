//! Expansion of `#[access_controlled_service]` (public docs: `lib.rs`,
//! `docs/server.md`).
//!
//! Output: the `impl` block with each method's policy attribute stripped, its
//! context parameter rewritten to
//! `::rust_grpc_lib::__private::tonic::Request<T>` and a context-building
//! prologue prepended, followed by
//! `impl ::rust_grpc_lib::auth::ServicePolicy<FooServer<Type>> for Type`
//! holding the policy table. The macro emits no access check: the gate that
//! `GrpcServer::add_service` installs builds a `PolicyTable` from this impl
//! and denies any method it does not list.
//!
//! Per method, in order: [`async_trait_order`] (first, so a misplaced
//! attribute is not misreported as a missing policy or a type error),
//! [`policy`] (strip and parse), [`request_param`] (rewrite and prologue).
//!
//! Errors raised here (`compile_fail_cases/`): not a trait `impl`; generic
//! arguments on `server = ..` or, when it is absent, on the trait path; no
//! policy (`missing_policy`); a macro invocation among the items; a
//! normalized-name collision (`colliding_method_names`). Argument, path and
//! block errors abort at once;
//! after that each method reports its first error, all methods' errors are
//! combined, and the collision check runs only if every method succeeded.
//!
//! Security invariants:
//! - Every `fn` has exactly one policy; a missing one is an error, never a
//!   default, and only `#[public]` yields `Access::Public`. A macro
//!   invocation in the block is an error too, since it could hide a handler.
//! - The table is built from exactly the stripped attributes and is bound to
//!   the generated server type, which `add_service` requires
//!   (`server_builder_unannotated_service`).
//! - Handlers cannot forge or bypass their context; see [`request_param`].
//! - Normalized names are unique: the gate takes the first match, so a
//!   collision would give one method another's policy. `#[cfg]` is not
//!   evaluated here, so `cfg`-gated variants of one method collide as well;
//!   the error says how to restructure.

use std::collections::HashMap;

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{Error, Ident, ImplItem, ImplItemFn, ItemImpl, Path, PathArguments};

use args::parse_server_arg;
use async_trait_order::check_async_trait_order;
use policy::{Policy, extract_policy};
use request_param::rewrite_context_param;

mod args;
mod async_trait_order;
mod policy;
mod request_param;
mod roles;
#[cfg(test)]
mod tests;

/// Used by the crate root's inert policy attributes.
pub(crate) use policy::Kind;

/// Entry point; see the module docs.
pub(crate) fn expand(attr: TokenStream2, item: TokenStream2) -> syn::Result<TokenStream2> {
    let explicit_server = parse_server_arg(attr)?;
    let mut impl_block: ItemImpl = syn::parse2(item)?;

    let server = server_path(explicit_server.as_ref(), &impl_block)?;
    let entries = expand_methods(&mut impl_block)?;
    check_collisions(&entries)?;

    let table = policy_impl(&impl_block, &server, &entries);
    Ok(quote! { #impl_block #table })
}

/// One method's row of the policy table.
struct TableEntry {
    name: Ident,
    policy: Policy,
    /// Has a `#[cfg]` or `#[cfg_attr]`, which this macro runs too early to
    /// evaluate; only used to explain a collision.
    cfg_gated: bool,
}

/// `explicit` if given, else the trait path with `Foo` renamed to tonic's
/// `FooServer`. Errors if the block is not a trait `impl` or the chosen path
/// has generic arguments.
fn server_path(explicit: Option<&Path>, impl_block: &ItemImpl) -> syn::Result<Path> {
    let Some((_, trait_path, _)) = &impl_block.trait_ else {
        return Err(Error::new_spanned(
            &impl_block.self_ty,
            "`#[access_controlled_service]` goes on an `impl Trait for Type` block of a tonic \
             service trait",
        ));
    };
    if let Some(server) = explicit {
        return without_generics(server);
    }
    let mut path = without_generics(trait_path)?;
    if let Some(last) = path.segments.last_mut() {
        last.ident = Ident::new(&format!("{}Server", last.ident), last.ident.span());
    }
    Ok(path)
}

/// Rejects generic arguments on the last segment: the macro supplies the
/// implementing type as the only argument.
fn without_generics(path: &Path) -> syn::Result<Path> {
    let mut path = path.clone();
    if let Some(last) = path.segments.last_mut() {
        if !last.arguments.is_empty() {
            return Err(Error::new_spanned(
                &last.arguments,
                "write the path without generic arguments; the macro supplies the implementing \
                 type",
            ));
        }
        last.arguments = PathArguments::None;
    }
    Ok(path)
}

/// Processes every `fn` item, combining errors. Associated types and consts
/// carry no policy and pass through. Macro invocations are rejected: they may
/// expand to a handler the macro cannot see, which would get no table row and
/// keep its raw `Request<T>`.
fn expand_methods(impl_block: &mut ItemImpl) -> syn::Result<Vec<TableEntry>> {
    let mut entries = Vec::new();
    let mut errors = Vec::new();
    for item in &mut impl_block.items {
        match item {
            ImplItem::Fn(method) => match process_method(method) {
                Ok(entry) => entries.push(entry),
                Err(err) => errors.push(err),
            },
            ImplItem::Macro(_) | ImplItem::Verbatim(_) => errors.push(opaque_item(item)),
            _ => {}
        }
    }
    combine_all(errors).map(|()| entries)
}

fn opaque_item(item: &ImplItem) -> Error {
    Error::new_spanned(
        item,
        "this item may define a method that `#[access_controlled_service]` cannot see, so it \
         would get no access policy; write the method out in the block",
    )
}

/// Rewrites one method in place and returns its row. Stops at the first error,
/// since each step needs the previous one (no policy, no context type).
fn process_method(method: &mut ImplItemFn) -> syn::Result<TableEntry> {
    check_async_trait_order(&method.sig)?;

    let Some(policy) = extract_policy(&mut method.attrs)? else {
        return Err(missing_policy(method));
    };

    let prologue = rewrite_context_param(&mut method.sig, policy.is_public())?;
    method.block.stmts.insert(0, prologue);

    Ok(TableEntry {
        name: method.sig.ident.clone(),
        policy,
        cfg_gated: method
            .attrs
            .iter()
            .any(|attr| attr.path().is_ident("cfg") || attr.path().is_ident("cfg_attr")),
    })
}

fn missing_policy(method: &ImplItemFn) -> Error {
    let name = &method.sig.ident;
    Error::new_spanned(
        name,
        format!(
            "method `{name}` declares no access policy; add {}",
            Kind::alternatives()
        ),
    )
}

/// Lowercase, without `_` or `r#`, so `getEndPointOne` (wire) and
/// `get_end_point_one` (Rust) match. Must stay identical to `same_method` in
/// `crates/core/src/auth/policy.rs`, which the gate uses for lookup; a
/// proc-macro crate cannot share it.
fn normalize(name: &str) -> String {
    name.trim_start_matches("r#")
        .chars()
        .filter(|c| *c != '_')
        .flat_map(char::to_lowercase)
        .collect()
}

/// Errors on each method whose normalized name equals an earlier one's.
fn check_collisions(entries: &[TableEntry]) -> syn::Result<()> {
    let mut seen: HashMap<String, &TableEntry> = HashMap::new();
    combine_all(entries.iter().filter_map(|entry| {
        let first = seen.insert(normalize(&entry.name.to_string()), entry)?;
        let hint = if entry.cfg_gated || first.cfg_gated {
            CFG_HINT
        } else {
            ""
        };
        Some(Error::new_spanned(
            &entry.name,
            format!(
                "method `{}` collides with `{}`: the access gate matches methods by their \
                 lowercase name without underscores, so it could not tell their policies \
                 apart{hint}",
                entry.name, first.name
            ),
        ))
    }))
}

/// Appended when a collision involves a `#[cfg]`-gated method, the one case
/// where the colliding methods might never be compiled in together.
const CFG_HINT: &str = "\nnote: `#[cfg]` on a method is not evaluated before this macro runs, so \
     it sees every variant even if only one is compiled in; to choose a variant by `cfg`, \
     write one `impl` block per variant and put `#[cfg(..)]` on each block, above \
     `#[access_controlled_service]`";

/// `POLICIES` lists methods in declaration order under their Rust name minus
/// `r#`, not normalized: the gate normalizes at lookup and the plain name
/// stays readable in `PolicyTable::policies`. Reuses the block's generics.
fn policy_impl(impl_block: &ItemImpl, server: &Path, entries: &[TableEntry]) -> TokenStream2 {
    let (impl_generics, _, where_clause) = impl_block.generics.split_for_impl();
    let self_ty = &impl_block.self_ty;
    let rows = entries.iter().map(|entry| {
        let name = entry.name.to_string();
        let name = name.trim_start_matches("r#");
        let access = entry.policy.access();
        quote! { ::rust_grpc_lib::auth::MethodPolicy::new(#name, #access) }
    });

    quote! {
        impl #impl_generics ::rust_grpc_lib::auth::ServicePolicy<#server<#self_ty>>
            for #self_ty #where_clause
        {
            const POLICIES: &'static [::rust_grpc_lib::auth::MethodPolicy] = &[#(#rows),*];
        }
    }
}

/// Combines errors so one compile shows them all; `Ok` if none.
fn combine_all(errors: impl IntoIterator<Item = Error>) -> syn::Result<()> {
    errors
        .into_iter()
        .reduce(|mut all, err| {
            all.combine(err);
            all
        })
        .map_or(Ok(()), Err)
}
