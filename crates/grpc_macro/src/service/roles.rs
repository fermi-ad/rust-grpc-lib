//! Parsing of the `#[roles(...)]` marker attribute.
//!
//! Two forms are accepted: `any("r1", "r2")` (at least one role must be held)
//! and `all("r1", "r2")` (every role must be held). The attribute is stripped
//! from the method here so it never reaches the compiler.

use std::mem::take;

use syn::{
    Attribute, Ident, LitStr, Meta, Token,
    parse::{Parse, ParseStream},
    punctuated::Punctuated,
};

/// The parsed content of a `#[roles(...)]` attribute.
pub(super) enum RolesSpec {
    Any(Vec<String>),
    All(Vec<String>),
}

impl RolesSpec {
    /// The roles the endpoint requires.
    pub(super) fn roles(&self) -> &[String] {
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
    /// Parses `any("r1", "r2")` or `all("r1", "r2")`.
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let variant: Ident = input.parse()?;

        let content;
        syn::parenthesized!(content in input);
        let literals = Punctuated::<LitStr, Token![,]>::parse_terminated(&content)?;
        let roles: Vec<String> = literals.iter().map(LitStr::value).collect();

        if !input.is_empty() {
            return Err(input.error("unexpected tokens after the role list"));
        }
        if roles.is_empty() {
            return Err(syn::Error::new_spanned(
                &variant,
                "at least one role is required",
            ));
        }

        match variant.to_string().as_str() {
            "any" => Ok(RolesSpec::Any(roles)),
            "all" => Ok(RolesSpec::All(roles)),
            other => Err(syn::Error::new_spanned(
                &variant,
                format!("expected `any` or `all`, found `{other}`"),
            )),
        }
    }
}

/// Removes the `#[roles(...)]` attribute from `attrs` and returns its parsed
/// [`RolesSpec`], or `None` if the method has no such attribute.
pub(super) fn extract_roles_attr(attrs: &mut Vec<Attribute>) -> syn::Result<Option<RolesSpec>> {
    let mut found = None;
    let mut remaining = Vec::with_capacity(attrs.len());
    for attr in take(attrs) {
        if !is_roles(&attr) {
            remaining.push(attr);
            continue;
        }
        if found.is_some() {
            return Err(syn::Error::new_spanned(&attr, "duplicate `#[roles]`"));
        }
        let Meta::List(list) = &attr.meta else {
            return Err(syn::Error::new_spanned(
                &attr,
                "expected `#[roles(any(..))]` or `#[roles(all(..))]`",
            ));
        };
        found = Some(syn::parse2(list.tokens.clone())?);
    }
    *attrs = remaining;
    Ok(found)
}

/// Returns `true` if the last segment of the attribute's path is `roles`.
fn is_roles(attr: &Attribute) -> bool {
    attr.path()
        .segments
        .last()
        .is_some_and(|seg| seg.ident == "roles")
}
