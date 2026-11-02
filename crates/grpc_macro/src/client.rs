//! Expansion of the `GrpcClient` and `GrpcNoAuthClient` derive macros.
//!
//! Both derives add a constructor to a tonic-generated client struct that
//! pulls a channel from the process-wide pool. The emitted code names
//! `::rust_grpc_lib` so it resolves against the core crate when expanded, not
//! against any re-export in a consuming crate.

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::DeriveInput;

/// Expands `#[derive(GrpcClient)]`: generates `from_endpoint_with_provider`,
/// which wires the pooled channel up with `ClientJwtInterceptor`.
pub(crate) fn expand_grpc_client(input: TokenStream2) -> syn::Result<TokenStream2> {
    let parsed: DeriveInput = syn::parse2(input)?;
    let name = &parsed.ident;

    Ok(quote! {
        impl #name<::tonic::transport::Channel>
        {
            pub fn from_endpoint_with_provider<P: ::rust_grpc_lib::auth::TokenProvider>(
                endpoint: &str,
                provider: P,
            ) -> Result<#name<::tonic::service::interceptor::InterceptedService<::tonic::transport::Channel, ::rust_grpc_lib::auth::ClientJwtInterceptor<P>>>, ::tonic::transport::Error>
            {
                let channel = ::rust_grpc_lib::pool::get_channel(endpoint)?;
                Ok(#name::new(::tonic::service::interceptor::InterceptedService::new(channel, ::rust_grpc_lib::auth::ClientJwtInterceptor::new(provider))))
            }
        }
    })
}

/// Expands `#[derive(GrpcNoAuthClient)]`: generates `from_endpoint`, which
/// returns the pooled channel without any auth wiring.
pub(crate) fn expand_grpc_no_auth_client(input: TokenStream2) -> syn::Result<TokenStream2> {
    let parsed: DeriveInput = syn::parse2(input)?;
    let name = &parsed.ident;

    Ok(quote! {
        impl #name<::tonic::transport::Channel>
        {
            pub fn from_endpoint(
                endpoint: &str,
            ) -> Result<Self, ::tonic::transport::Error>
            {
                let channel = ::rust_grpc_lib::pool::get_channel(endpoint)?;
                Ok(#name::new(channel))
            }
        }
    })
}
