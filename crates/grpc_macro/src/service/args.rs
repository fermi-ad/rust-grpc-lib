//! Parses `#[access_controlled_service(server = <path>)]`, the only argument.
//!
//! Errors: unknown argument, duplicate `server`, malformed value.

use proc_macro2::TokenStream as TokenStream2;
use quote::ToTokens;
use syn::{Path, meta, parse::Parser};

#[cfg(test)]
mod tests;

/// `None` if no argument is given. The path is only parsed: rustc resolves it
/// in the generated impl, and `without_generics` rejects generic arguments.
pub(super) fn parse_server_arg(attr: TokenStream2) -> syn::Result<Option<Path>> {
    let mut server = None;
    meta::parser(|meta| {
        if !meta.path.is_ident("server") {
            return Err(meta.error(format!(
                "unknown argument `{}`; the only argument is `server = <path>`",
                meta.path.to_token_stream()
            )));
        }
        if server.is_some() {
            return Err(meta.error("duplicate `server` argument"));
        }
        server = Some(meta.value()?.parse::<Path>()?);
        Ok(())
    })
    .parse2(attr)?;
    Ok(server)
}
