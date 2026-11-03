//! The arguments of `#[keycloak_authenticated_service(...)]`.
//!
//! Two arguments are accepted, separated by commas and in any order:
//!
//! - `claims = <path>` selects the claims type that `Request<T>` methods read
//!   from the request extensions.
//! - `strict` requires every method to declare an access policy.

use syn::{
    Error, Ident, Path, Token,
    parse::{Parse, ParseStream},
};

#[cfg(test)]
mod tests;

/// Parsed attribute arguments.
pub(super) struct MacroArgs {
    /// The claims type from `claims = <path>`, if given.
    pub(super) claims: Option<Path>,
    /// Whether `strict` was given.
    pub(super) strict: bool,
}

impl Parse for MacroArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let mut claims = None;
        let mut strict = false;
        while !input.is_empty() {
            let key: Ident = input.parse()?;
            match key.to_string().as_str() {
                "claims" => {
                    if claims.is_some() {
                        return Err(Error::new_spanned(&key, "duplicate `claims` argument"));
                    }
                    input.parse::<Token![=]>()?;
                    claims = Some(input.parse::<Path>()?);
                }
                "strict" => {
                    if strict {
                        return Err(Error::new_spanned(&key, "duplicate `strict` argument"));
                    }
                    strict = true;
                }
                _ => {
                    return Err(Error::new_spanned(
                        &key,
                        format!("unknown argument `{key}`; expected `claims = <path>` or `strict`"),
                    ));
                }
            }
            if !input.is_empty() {
                input.parse::<Token![,]>()?;
            }
        }
        Ok(Self { claims, strict })
    }
}
