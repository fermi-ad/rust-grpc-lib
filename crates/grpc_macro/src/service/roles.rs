//! Parsing of the argument of `#[roles(...)]`.
//!
//! Two forms are accepted: `any(r1, r2)` (at least one role must be held) and
//! `all(r1, r2)` (every role must be held). Each role is either a string
//! literal (`"operator"`) or a path to a `&str` constant (`roles::OPERATOR`),
//! so a consumer can keep its role names in one module.
//!
//! Locating and stripping the attribute itself is the job of `policy`.

use proc_macro2::TokenStream as TokenStream2;
use quote::ToTokens;
use syn::{
    Error, Ident, LitStr, Path, Token, parenthesized,
    parse::{Parse, ParseStream},
    punctuated::Punctuated,
};

#[cfg(test)]
mod tests;

/// The parsed argument of a `#[roles(...)]` attribute.
pub(super) enum RolesSpec {
    Any(Vec<RoleItem>),
    All(Vec<RoleItem>),
}

impl RolesSpec {
    /// The roles the endpoint requires.
    pub(super) fn roles(&self) -> &[RoleItem] {
        match self {
            RolesSpec::Any(roles) | RolesSpec::All(roles) => roles,
        }
    }

    /// `"any"` or `"all"`: the name of the iterator check the guard runs and
    /// the `role_mode` reported in denial logs.
    pub(super) fn mode(&self) -> &'static str {
        match self {
            RolesSpec::Any(_) => "any",
            RolesSpec::All(_) => "all",
        }
    }
}

impl Parse for RolesSpec {
    /// Parses `any(r1, r2)` or `all(r1, r2)`.
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let variant: Ident = input.parse()?;

        let content;
        parenthesized!(content in input);
        let items = Punctuated::<RoleItem, Token![,]>::parse_terminated(&content)?;
        let roles: Vec<RoleItem> = items.into_iter().collect();

        if !input.is_empty() {
            return Err(input.error("unexpected tokens after the role list"));
        }
        if roles.is_empty() {
            return Err(Error::new_spanned(
                &variant,
                "at least one role is required",
            ));
        }

        match variant.to_string().as_str() {
            "any" => Ok(RolesSpec::Any(roles)),
            "all" => Ok(RolesSpec::All(roles)),
            other => Err(Error::new_spanned(
                &variant,
                format!("expected `any` or `all`, found `{other}`"),
            )),
        }
    }
}

/// One required role as written in the attribute.
pub(super) enum RoleItem {
    /// A string literal, such as `"operator"`.
    Literal(LitStr),
    /// A path to a `&str` constant, such as `roles::OPERATOR`. Its type is
    /// checked by the compiler where the guard uses it.
    Path(Path),
}

impl Parse for RoleItem {
    /// Parses a string literal or a path.
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        if input.peek(LitStr) {
            return input.parse().map(RoleItem::Literal);
        }
        input.parse().map(RoleItem::Path).map_err(|_| {
            input.error(
                "expected a string literal or a path to a `&str` constant, \
                 such as `\"operator\"` or `roles::OPERATOR`",
            )
        })
    }
}

impl ToTokens for RoleItem {
    fn to_tokens(&self, tokens: &mut TokenStream2) {
        match self {
            RoleItem::Literal(literal) => literal.to_tokens(tokens),
            RoleItem::Path(path) => path.to_tokens(tokens),
        }
    }
}
