//! Tests for the `#[async_trait]` order check.

use super::*;

use syn::{ItemFn, parse_quote};

fn signature(item: ItemFn) -> Signature {
    item.sig
}

#[test]
fn an_async_fn_is_accepted() {
    let sig = signature(parse_quote! {
        async fn echo(&self) {}
    });

    assert!(check_async_trait_order(&sig).is_ok());
}

#[test]
fn a_plain_fn_without_the_async_trait_lifetime_is_accepted() {
    let sig = signature(parse_quote! {
        fn echo<'a>(&'a self) {}
    });

    assert!(check_async_trait_order(&sig).is_ok());
}

#[test]
fn a_method_already_rewritten_by_async_trait_is_rejected() {
    let sig = signature(parse_quote! {
        fn echo<'life0, 'async_trait>(&'life0 self) where 'life0: 'async_trait {}
    });

    let message = check_async_trait_order(&sig)
        .expect_err("the method must be rejected")
        .to_string();

    assert!(
        message.starts_with("`#[access_controlled_service]` must be placed above"),
        "{message}"
    );
    assert!(
        message.contains("must be placed above `#[async_trait]`"),
        "{message}"
    );
}
