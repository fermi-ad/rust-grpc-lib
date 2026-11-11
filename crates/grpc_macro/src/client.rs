//! Expansion of the `GrpcClient` derive.
//!
//! Input: a struct, enum or union; any other item is a `syn` parse error. Only
//! its name is used; generics and attributes are dropped. The generated code is
//! an `impl Name<::rust_grpc_lib::__private::tonic::transport::Channel>`
//! calling `Name::new(inner)`, so `Name` needs one type parameter and that
//! constructor, as tonic clients have. Every path starts at `::rust_grpc_lib`,
//! so the consuming crate needs no `tonic` dependency of its own.

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{DeriveInput, parse2};

#[cfg(test)]
mod tests;

/// `#[derive(GrpcClient)]`: emits two constructors.
///
/// - `public(endpoint)`: takes a pooled channel and returns `Name::new(channel)`,
///   which sends no token.
/// - `authenticated(endpoint, provider)`: takes a pooled channel and wraps it in
///   `AuthenticatedClient` with a `ClientJwtInterceptor`.
pub(crate) fn expand_grpc_client(input: TokenStream2) -> syn::Result<TokenStream2> {
    let DeriveInput { ident: name, .. } = parse2(input)?;

    let public_doc = "Connects to `endpoint` without a token.\n\n\
        Use it to call methods the server marks `#[public]`. Methods behind an \
        access policy answer `Unauthenticated`; use `authenticated` for those.";
    let authenticated_doc = "Connects to `endpoint` and attaches the token from `provider` \
        to every request.\n\n\
        Use it to call methods behind an access policy (`#[roles]` or \
        `#[any_authenticated]`).";

    Ok(quote! {
        impl #name<::rust_grpc_lib::__private::tonic::transport::Channel>
        {
            #[doc = #public_doc]
            pub fn public(
                endpoint: &str,
            ) -> ::core::result::Result<Self, ::rust_grpc_lib::pool::ChannelError>
            {
                let channel = ::rust_grpc_lib::pool::get_channel(endpoint)?;
                ::core::result::Result::Ok(#name::new(channel))
            }

            #[doc = #authenticated_doc]
            pub fn authenticated<P: ::rust_grpc_lib::auth::TokenProvider>(
                endpoint: &str,
                provider: P,
            ) -> ::core::result::Result<#name<::rust_grpc_lib::auth::AuthenticatedClient<P>>, ::rust_grpc_lib::pool::ChannelError>
            {
                let channel = ::rust_grpc_lib::pool::get_channel(endpoint)?;
                ::core::result::Result::Ok(#name::new(::rust_grpc_lib::auth::AuthenticatedClient::<P>::new(
                    channel,
                    ::rust_grpc_lib::auth::ClientJwtInterceptor::new(provider),
                )))
            }
        }
    })
}
