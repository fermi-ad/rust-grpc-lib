use proc_macro::TokenStream;
use quote::quote;
use syn::{DeriveInput, parse_macro_input};

/// Derive macro that implements the `GrpcClient` trait for a tonic client struct.
///
/// This macro is applied automatically by `rust-grpc-lib`'s build script to every
/// generated tonic client type, so you will not normally need to use it yourself.
/// It is re-exported from the main crate as `rust_grpc_lib::GrpcClient` for the
/// rare case where you need to implement the trait on a custom wrapper type.
///
/// # Example
///
/// ```rust,ignore
/// use rust_grpc_lib::GrpcClient;
///
/// #[derive(GrpcClient)]
/// pub struct MyServiceClient<T>(inner_client::MyServiceClient<T>);
/// ```
#[proc_macro_derive(GrpcClient)]
pub fn grpc_client_derive(input: TokenStream) -> TokenStream {
    let parsed = parse_macro_input!(input as DeriveInput);
    let name = &parsed.ident;

    // Use `::rust_grpc_lib` so the impl resolves against the `core` crate when this
    // derive is expanded, not against any re-export in a consuming crate.
    let as_grpc_client = quote! {
        impl ::rust_grpc_lib::GrpcClient for #name<::tonic::transport::Channel> {
            fn from_channel(ch: ::tonic::transport::Channel) -> Self {
                <#name<::tonic::transport::Channel>>::new(ch)
            }
        }
    };
    TokenStream::from(as_grpc_client)
}
