//! Tests for the client derive expansion.

use super::*;

use syn::{File, ImplItem, ImplItemFn, Item, ItemImpl};

fn client_struct() -> TokenStream2 {
    quote! {
        pub struct ExampleClient<T> {
            inner: T,
        }
    }
}

/// The single `impl` block of an expansion; other items are skipped.
fn impl_block(expanded: TokenStream2) -> ItemImpl {
    let mut impls = parse2::<File>(expanded)
        .expect("expansion is a list of items")
        .items
        .into_iter()
        .filter_map(|item| match item {
            Item::Impl(item) => Some(item),
            _ => None,
        });
    let item = impls.next().expect("expansion has an impl block");
    assert!(impls.next().is_none(), "expansion has one impl block");
    item
}

fn methods(expanded: TokenStream2) -> Vec<ImplItemFn> {
    impl_block(expanded)
        .items
        .into_iter()
        .filter_map(|item| match item {
            ImplItem::Fn(method) => Some(method),
            _ => None,
        })
        .collect()
}

fn method(expanded: TokenStream2, name: &str) -> ImplItemFn {
    methods(expanded)
        .into_iter()
        .find(|method| method.sig.ident == name)
        .unwrap_or_else(|| panic!("expansion has a `{name}` method"))
}

/// Token text with whitespace removed, for comparing types and signatures.
fn compact<T: quote::ToTokens>(tokens: &T) -> String {
    tokens.to_token_stream().to_string().replace(' ', "")
}

/// The text of the `#[doc = "..."]` attribute of a method.
fn doc_text(method: &ImplItemFn) -> String {
    let docs: Vec<String> = method
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident("doc"))
        .map(|attr| match &attr.meta {
            syn::Meta::NameValue(syn::MetaNameValue {
                value:
                    syn::Expr::Lit(syn::ExprLit {
                        lit: syn::Lit::Str(text),
                        ..
                    }),
                ..
            }) => text.value(),
            other => panic!("doc attribute is not a string: {}", compact(other)),
        })
        .collect();
    assert_eq!(docs.len(), 1, "exactly one doc attribute");
    docs.into_iter().next().unwrap()
}

#[test]
fn grpc_client_generates_public_and_authenticated() {
    let expanded = expand_grpc_client(client_struct()).expect("expansion succeeds");

    let names: Vec<String> = methods(expanded)
        .iter()
        .map(|method| method.sig.ident.to_string())
        .collect();
    assert_eq!(names, ["public", "authenticated"]);
}

#[test]
fn grpc_client_impl_targets_the_channel_instantiation() {
    let expanded = expand_grpc_client(client_struct()).expect("expansion succeeds");

    let block = impl_block(expanded);
    assert!(block.trait_.is_none(), "inherent impl");
    assert_eq!(
        compact(&block.self_ty),
        "ExampleClient<::rust_grpc_lib::__private::tonic::transport::Channel>"
    );
}

#[test]
fn public_takes_an_endpoint_and_returns_self_or_channel_error() {
    let expanded = expand_grpc_client(client_struct()).expect("expansion succeeds");

    let public = method(expanded, "public");
    assert!(matches!(public.vis, syn::Visibility::Public(_)));
    assert!(public.sig.generics.params.is_empty(), "no generics");
    assert!(public.sig.asyncness.is_none());
    assert_eq!(
        compact(&public.sig.inputs).trim_end_matches(','),
        "endpoint:&str",
        "unexpected parameters"
    );
    assert_eq!(
        compact(&public.sig.output),
        "->::core::result::Result<Self,::rust_grpc_lib::pool::ChannelError>"
    );
}

#[test]
fn public_wraps_the_pooled_channel_without_a_token() {
    let expanded = expand_grpc_client(client_struct()).expect("expansion succeeds");

    let body = compact(&method(expanded, "public").block);
    assert!(
        body.contains("::rust_grpc_lib::pool::get_channel(endpoint)?"),
        "unexpected body: {body}"
    );
    assert!(
        body.contains("ExampleClient::new(channel)"),
        "unexpected body: {body}"
    );
    assert!(
        !body.contains("AuthenticatedClient") && !body.contains("ClientJwtInterceptor"),
        "public must not attach a token: {body}"
    );
}

#[test]
fn authenticated_takes_endpoint_and_provider_and_returns_an_authenticated_client() {
    let expanded = expand_grpc_client(client_struct()).expect("expansion succeeds");

    let authenticated = method(expanded, "authenticated");
    assert!(matches!(authenticated.vis, syn::Visibility::Public(_)));
    assert!(authenticated.sig.asyncness.is_none());
    assert_eq!(
        compact(&authenticated.sig.generics),
        "<P:::rust_grpc_lib::auth::TokenProvider>"
    );
    assert_eq!(
        compact(&authenticated.sig.inputs).trim_end_matches(','),
        "endpoint:&str,provider:P",
        "unexpected parameters"
    );
    assert_eq!(
        compact(&authenticated.sig.output),
        "->::core::result::Result<\
         ExampleClient<::rust_grpc_lib::auth::AuthenticatedClient<P>>,\
         ::rust_grpc_lib::pool::ChannelError>"
    );
}

#[test]
fn authenticated_wraps_the_pooled_channel_with_the_jwt_interceptor() {
    let expanded = expand_grpc_client(client_struct()).expect("expansion succeeds");

    let body = compact(&method(expanded, "authenticated").block);
    assert!(
        body.contains("::rust_grpc_lib::pool::get_channel(endpoint)?"),
        "unexpected body: {body}"
    );
    assert!(
        body.contains(
            "ExampleClient::new(::rust_grpc_lib::auth::AuthenticatedClient::<P>::new(\
             channel,::rust_grpc_lib::auth::ClientJwtInterceptor::new(provider),))"
        ),
        "unexpected body: {body}"
    );
}

#[test]
fn each_constructor_has_a_doc_comment() {
    let expanded = expand_grpc_client(client_struct()).expect("expansion succeeds");

    let public = doc_text(&method(expanded.clone(), "public"));
    assert!(
        public.contains("without a token"),
        "unexpected doc: {public}"
    );
    assert!(public.contains("#[public]"), "unexpected doc: {public}");

    let authenticated = doc_text(&method(expanded, "authenticated"));
    assert!(
        authenticated.contains("token from `provider`"),
        "unexpected doc: {authenticated}"
    );
    assert!(
        authenticated.contains("every request"),
        "unexpected doc: {authenticated}"
    );

    for doc in [&public, &authenticated] {
        let lower = doc.to_lowercase();
        assert!(
            !lower.contains("tests only") && !lower.contains("warning"),
            "doc must not use warning-style wording: {doc}"
        );
    }
}

#[test]
fn grpc_client_uses_only_the_type_name() {
    // Generics and attributes on the input must not leak into the expansion.
    let expanded = expand_grpc_client(quote! {
        #[derive(Debug)]
        pub struct WrappedClient<T> where T: Clone {
            inner: T,
        }
    })
    .expect("expansion succeeds");

    let block = impl_block(expanded);
    assert_eq!(
        compact(&block.self_ty),
        "WrappedClient<::rust_grpc_lib::__private::tonic::transport::Channel>"
    );
    assert!(block.generics.params.is_empty());
    assert!(block.generics.where_clause.is_none());
}

#[test]
fn grpc_client_rejects_input_that_is_not_a_type() {
    let error = expand_grpc_client(quote! { fn not_a_type() {} });

    assert!(error.is_err());
}
