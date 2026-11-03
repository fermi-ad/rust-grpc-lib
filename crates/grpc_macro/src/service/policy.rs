//! The access policy a method declares.
//!
//! A method is in one of two policy states, or declares none:
//!
//! - `#[roles(any(..))]` / `#[roles(all(..))]`: the caller must hold the listed
//!   roles.
//! - `#[any_authenticated]`: any caller with valid claims is allowed, no role is
//!   needed.
//!
//! [`extract_policy`] finds the attribute, strips it so it never reaches the
//! compiler, and parses it. A method may declare at most one policy. In
//! `strict` mode a method with none is a compile error (reported by `service`).
//!
//! [`Kind`] is the single list of policy attributes: their names, how the
//! error messages spell them, and the error for one that was never stripped.
//! Adding a policy means adding a variant there and a variant of [`Policy`].

use std::mem::take;

use proc_macro2::Span;
use syn::{Attribute, Error, Meta};

use super::roles::RolesSpec;

#[cfg(test)]
mod tests;

/// Which policy attribute an [`Attribute`] is, by the last segment of its path.
#[derive(Clone, Copy)]
pub(crate) enum Kind {
    /// `#[roles(..)]`.
    Roles,
    /// `#[any_authenticated]`.
    AnyAuthenticated,
}

impl Kind {
    /// Every policy attribute.
    const ALL: [Self; 2] = [Self::Roles, Self::AnyAuthenticated];

    /// The error for this attribute reaching its own expansion, which means
    /// `#[keycloak_authenticated_service]` did not strip it: it was written on
    /// something other than a method, or in a block that is not annotated.
    pub(crate) fn unstripped_error(self) -> Error {
        let name = self.name();
        Error::new(
            Span::call_site(),
            format!(
                "`#[{name}]` has no effect here: it must be written on a method (`fn`) \
                 inside an `impl` block annotated with `#[keycloak_authenticated_service]`, \
                 which must be written above `#[async_trait]`"
            ),
        )
    }

    /// Every policy, spelled for an error message and joined with "or".
    pub(super) fn alternatives() -> String {
        Self::ALL.map(Self::usage).join(" or ")
    }

    /// Classifies an attribute, or returns `None` if it is not a policy.
    fn of(attr: &Attribute) -> Option<Self> {
        let segment = attr.path().segments.last()?;
        Self::ALL
            .into_iter()
            .find(|kind| segment.ident == kind.name())
    }

    /// The attribute's name, as written after `#[`.
    fn name(self) -> &'static str {
        match self {
            Self::Roles => "roles",
            Self::AnyAuthenticated => "any_authenticated",
        }
    }

    /// The attribute as an error message shows it, in code quotes.
    fn usage(self) -> &'static str {
        match self {
            Self::Roles => "`#[roles(..)]`",
            Self::AnyAuthenticated => "`#[any_authenticated]`",
        }
    }
}

/// The policy declared on a method.
pub(super) enum Policy {
    /// `#[roles(..)]`.
    Roles(RolesSpec),
    /// `#[any_authenticated]`.
    AnyAuthenticated,
}

/// Removes the policy attribute from `attrs` and returns its parsed
/// [`Policy`], or `None` if the method declares none.
///
/// # Errors
///
/// Returns an error if the method declares more than one policy, if an
/// attribute is malformed, or if a marker attribute is given arguments.
pub(super) fn extract_policy(attrs: &mut Vec<Attribute>) -> syn::Result<Option<Policy>> {
    let mut found = None;
    let mut remaining = Vec::with_capacity(attrs.len());
    for attr in take(attrs) {
        let Some(kind) = Kind::of(&attr) else {
            remaining.push(attr);
            continue;
        };
        if found.is_some() {
            return Err(Error::new_spanned(
                &attr,
                format!(
                    "a method declares exactly one access policy: {}",
                    Kind::alternatives()
                ),
            ));
        }
        found = Some(parse_policy(kind, &attr)?);
    }
    *attrs = remaining;
    Ok(found)
}

/// Parses the policy an attribute of the given kind declares.
fn parse_policy(kind: Kind, attr: &Attribute) -> syn::Result<Policy> {
    match kind {
        Kind::Roles => {
            let Meta::List(list) = &attr.meta else {
                return Err(Error::new_spanned(
                    attr,
                    "expected `#[roles(any(..))]` or `#[roles(all(..))]`",
                ));
            };
            syn::parse2(list.tokens.clone()).map(Policy::Roles)
        }
        Kind::AnyAuthenticated => {
            if matches!(attr.meta, Meta::Path(_)) {
                Ok(Policy::AnyAuthenticated)
            } else {
                Err(Error::new_spanned(
                    attr,
                    "`#[any_authenticated]` takes no arguments",
                ))
            }
        }
    }
}
