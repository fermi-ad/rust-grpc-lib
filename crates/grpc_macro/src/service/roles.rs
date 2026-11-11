//! Parses the inside of `#[roles(..)]`: `any(..)` or `all(..)` over string
//! literals or paths to `&str` constants.
//!
//! Roles are re-emitted as written, so rustc resolves and type-checks path
//! roles in the table. The mode names the `Access::any_of`/`all_of`
//! constructor. An empty list is rejected here so the error points at the
//! attribute; the constructors' own assertion is the backstop.
//!
//! Errors: a mode other than `any`/`all` (`bad_role_variant`), an empty list,
//! a role that is neither literal nor path, trailing tokens.

use proc_macro2::TokenStream as TokenStream2;
use quote::{ToTokens, format_ident};
use syn::{
    Error, Ident, LitStr, Path, Token, parenthesized,
    parse::{Parse, ParseStream},
    punctuated::Punctuated,
};

#[cfg(test)]
mod tests;

pub(super) struct RolesSpec {
    /// `any_of` or `all_of`.
    pub(super) constructor: Ident,
    /// The roles' tokens as written.
    pub(super) roles: Vec<TokenStream2>,
}

impl Parse for RolesSpec {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let variant: Ident = input.parse()?;

        let content;
        parenthesized!(content in input);
        let roles =
            Punctuated::<TokenStream2, Token![,]>::parse_terminated_with(&content, parse_role)?
                .into_iter()
                .collect::<Vec<_>>();

        if !input.is_empty() {
            return Err(input.error("unexpected tokens after the role list"));
        }
        if roles.is_empty() {
            return Err(Error::new_spanned(
                &variant,
                "at least one role is required",
            ));
        }
        if variant != "any" && variant != "all" {
            return Err(Error::new_spanned(
                &variant,
                format!("expected `any` or `all`, found `{variant}`"),
            ));
        }

        Ok(Self {
            constructor: format_ident!("{variant}_of"),
            roles,
        })
    }
}

/// A string literal or a path (a bare imported constant is a one-segment
/// path).
fn parse_role(input: ParseStream<'_>) -> syn::Result<TokenStream2> {
    if input.peek(LitStr) {
        return input.parse::<LitStr>().map(ToTokens::into_token_stream);
    }
    input
        .parse::<Path>()
        .map(ToTokens::into_token_stream)
        .map_err(|_| {
            input.error(
                "expected a string literal or a path to a `&str` constant, \
                 such as `\"operator\"` or `roles::OPERATOR`",
            )
        })
}
