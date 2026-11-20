//! Strips and parses a method's policy attribute (`#[roles(..)]`,
//! `#[any_authenticated]`, `#[public]`) and renders its `Access` value.
//!
//! The attributes are real macros in the crate root that expand to
//! [`Kind::unstripped_error`], so one left outside an annotated block is an
//! error, not ignored (`policy_outside_service`, `public_outside_service`).
//! Stripping them here prevents that inside the block and makes the table
//! exactly the declared policies. They are matched by last path segment, so
//! `#[rust_grpc_lib::public]` counts.
//!
//! Errors: a second policy (`duplicate_policy`), a malformed `#[roles]`
//! (plus [`roles`](super::roles) errors), arguments on a bare attribute.
//!
//! To add a policy, add a `Kind` and a `Policy` variant; the compiler flags
//! each match to update.

use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::quote;
use syn::{Attribute, Error, Meta};

use super::roles::RolesSpec;

#[cfg(test)]
mod tests;

/// The policy attributes, by name; the single list used for matching and in
/// error messages.
#[derive(Clone, Copy)]
pub(crate) enum Kind {
    Roles,
    AnyAuthenticated,
    Public,
}

impl Kind {
    const ALL: [Self; 3] = [Self::Roles, Self::AnyAuthenticated, Self::Public];

    /// Emitted by the attribute's own expansion, which only runs when it was
    /// not stripped, i.e. it is outside an annotated block.
    pub(crate) fn unstripped_error(self) -> Error {
        let name = self.name();
        Error::new(
            Span::call_site(),
            format!(
                "`#[{name}]` has no effect here: it must be written on a method (`fn`) \
                 inside an `impl` block annotated with `#[access_controlled_service]`, \
                 which must be written above `#[async_trait]`"
            ),
        )
    }

    /// `` `#[roles(..)]` or `#[any_authenticated]` or `#[public]` ``.
    pub(super) fn alternatives() -> String {
        Self::ALL.map(Self::usage).join(" or ")
    }

    fn of(attr: &Attribute) -> Option<Self> {
        let segment = attr.path().segments.last()?;
        Self::ALL
            .into_iter()
            .find(|kind| segment.ident == kind.name())
    }

    fn name(self) -> &'static str {
        match self {
            Self::Roles => "roles",
            Self::AnyAuthenticated => "any_authenticated",
            Self::Public => "public",
        }
    }

    fn usage(self) -> &'static str {
        match self {
            Self::Roles => "`#[roles(..)]`",
            Self::AnyAuthenticated => "`#[any_authenticated]`",
            Self::Public => "`#[public]`",
        }
    }
}

/// A parsed policy attribute, one variant per [`Kind`].
pub(super) enum Policy {
    Roles(RolesSpec),
    AnyAuthenticated,
    Public,
}

impl Policy {
    /// `#[public]` methods take `PublicContext<T>`, others `RequestContext<T>`.
    pub(super) fn is_public(&self) -> bool {
        matches!(self, Self::Public)
    }

    /// The `::rust_grpc_lib::auth::Access` expression for the table. Only
    /// `Public` yields `Access::Public`. `any_of`/`all_of` are `const fn`s
    /// asserting a non-empty list, checked when the `const` table is evaluated.
    pub(super) fn access(&self) -> TokenStream2 {
        match self {
            Self::Public => quote! { ::rust_grpc_lib::auth::Access::Public },
            Self::AnyAuthenticated => quote! { ::rust_grpc_lib::auth::Access::Authenticated },
            Self::Roles(RolesSpec { constructor, roles }) => {
                quote! { ::rust_grpc_lib::auth::Access::#constructor(&[#(#roles),*]) }
            }
        }
    }
}

/// Removes every policy attribute from `attrs` and parses the first; `None`
/// if there is none. A second one is an error, spanned on it.
pub(super) fn extract_policy(attrs: &mut Vec<Attribute>) -> syn::Result<Option<Policy>> {
    // Collect to exhaust `extract_if`, which keeps any unvisited items.
    let declared: Vec<(Kind, Attribute)> = attrs
        .extract_if(.., |attr| Kind::of(attr).is_some())
        .filter_map(|attr| Some((Kind::of(&attr)?, attr)))
        .collect();

    let mut declared = declared.into_iter();
    let Some((kind, attr)) = declared.next() else {
        return Ok(None);
    };
    let policy = parse_policy(kind, &attr)?;
    if let Some((_, second)) = declared.next() {
        return Err(Error::new_spanned(
            &second,
            format!(
                "a method declares exactly one access policy: {}",
                Kind::alternatives()
            ),
        ));
    }
    Ok(Some(policy))
}

fn parse_policy(kind: Kind, attr: &Attribute) -> syn::Result<Policy> {
    match kind {
        Kind::Roles => {
            let Meta::List(list) = &attr.meta else {
                return Err(Error::new_spanned(
                    attr,
                    "expected `#[roles(any(..))]` or `#[roles(all(..))]`",
                ));
            };
            list.parse_args().map(Policy::Roles)
        }
        // The remaining policies are bare attributes.
        _ if !matches!(attr.meta, Meta::Path(_)) => Err(Error::new_spanned(
            attr,
            format!("`#[{}]` takes no arguments", kind.name()),
        )),
        Kind::AnyAuthenticated => Ok(Policy::AnyAuthenticated),
        Kind::Public => Ok(Policy::Public),
    }
}
