//! The arguments of `#[keycloak_authenticated_service(...)]`.
//!
//! The only argument today is `claims = <path>`, which selects the claims type
//! that `Request<T>` methods read from the request extensions.

use syn::{
    Ident, Path, Token,
    parse::{Parse, ParseStream},
};

/// Parsed attribute arguments.
pub(super) struct MacroArgs {
    /// The claims type from `claims = <path>`, if given.
    pub(super) claims: Option<Path>,
}

impl Parse for MacroArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let mut claims = None;
        while !input.is_empty() {
            let key: Ident = input.parse()?;
            if key != "claims" {
                return Err(syn::Error::new_spanned(
                    &key,
                    format!("unknown argument `{key}`; expected `claims = <path>`"),
                ));
            }
            if claims.is_some() {
                return Err(syn::Error::new_spanned(&key, "duplicate `claims` argument"));
            }
            input.parse::<Token![=]>()?;
            claims = Some(input.parse::<Path>()?);
            if !input.is_empty() {
                input.parse::<Token![,]>()?;
            }
        }
        Ok(Self { claims })
    }
}
