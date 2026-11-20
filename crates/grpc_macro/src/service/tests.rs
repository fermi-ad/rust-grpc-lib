//! Tests for the expansion of `#[access_controlled_service]`.

use super::*;

fn expand_ok(attr: TokenStream2, item: TokenStream2) -> String {
    expand(attr, item)
        .expect("the block must expand")
        .to_string()
}

fn expand_err(attr: TokenStream2, item: TokenStream2) -> String {
    expand(attr, item)
        .expect_err("the block must be rejected")
        .to_string()
}

/// A block with one method of each policy.
fn sample() -> TokenStream2 {
    quote! {
        impl Echo for Server {
            #[roles(any("admin", "operator"))]
            async fn get_end_point_one(&self, ctx: RequestContext<Req>) -> Out { 1 }

            #[any_authenticated]
            async fn get_end_point_two(&self, ctx: RequestContext<Req>) -> Out { 2 }

            #[public]
            async fn ping(&self, ctx: PublicContext<Req>) -> Out { 3 }
        }
    }
}

#[test]
fn the_table_lists_every_method_with_its_policy() {
    let out = expand_ok(quote! {}, sample()).replace(' ', "");

    assert!(
        out.contains("ServicePolicy<EchoServer<Server>>forServer"),
        "{out}"
    );
    assert!(
        out.contains(
            "MethodPolicy::new(\"get_end_point_one\",::rust_grpc_lib::auth::Access::any_of(&[\"admin\",\"operator\"]))"
        ),
        "{out}"
    );
    assert!(
        out.contains(
            "MethodPolicy::new(\"get_end_point_two\",::rust_grpc_lib::auth::Access::Authenticated)"
        ),
        "{out}"
    );
    assert!(
        out.contains("MethodPolicy::new(\"ping\",::rust_grpc_lib::auth::Access::Public)"),
        "{out}"
    );
}

#[test]
fn the_expansion_has_no_access_check_and_no_logging() {
    let out = expand_ok(quote! {}, sample());

    for forbidden in ["has_role", "permission_denied", "tracing", "authz"] {
        assert!(!out.contains(forbidden), "{forbidden} in {out}");
    }
}

#[test]
fn the_server_is_derived_from_a_qualified_trait_path() {
    let out = expand_ok(
        quote! {},
        quote! {
            impl example_db_server::ExampleDb for Stub {
                #[public]
                async fn ping(&self, ctx: PublicContext<Req>) -> Out { 1 }
            }
        },
    )
    .replace(' ', "");

    assert!(
        out.contains("ServicePolicy<example_db_server::ExampleDbServer<Stub>>forStub"),
        "{out}"
    );
}

#[test]
fn an_explicit_server_overrides_the_derived_one() {
    let out = expand_ok(
        quote! { server = other::CustomServer },
        quote! {
            impl Echo for Server {
                #[public]
                async fn ping(&self, ctx: PublicContext<Req>) -> Out { 1 }
            }
        },
    )
    .replace(' ', "");

    assert!(
        out.contains("ServicePolicy<other::CustomServer<Server>>forServer"),
        "{out}"
    );
}

#[test]
fn raw_identifier_method_names_lose_the_prefix() {
    let out = expand_ok(
        quote! {},
        quote! {
            impl Echo for Server {
                #[public]
                async fn r#type(&self, ctx: PublicContext<Req>) -> Out { 1 }
            }
        },
    )
    .replace(' ', "");

    assert!(out.contains("MethodPolicy::new(\"type\""), "{out}");
}

#[test]
fn every_bad_method_is_reported_in_one_expansion() {
    let err = expand(
        quote! {},
        quote! {
            impl Echo for Server {
                async fn a(&self, ctx: RequestContext<Req>) -> Out { 1 }
                async fn b(&self, ctx: RequestContext<Req>) -> Out { 2 }
            }
        },
    )
    .expect_err("the block must be rejected");

    assert_eq!(err.into_iter().count(), 2);
}

// Mirrors `method_names_compare_ignoring_case_and_underscores` in
// `crates/core/src/auth/policy/tests.rs` (`same_method`, the runtime twin of
// `normalize`) and the end-to-end `crates/integration_tests/src/method_name_matching.rs`.
// A Rust name and its proto spelling must normalize to the same string.
#[test]
fn normalize_matches_what_the_gate_compares() {
    for (rust, wire, normalized) in [
        ("get_end_point_one", "getEndPointOne", "getendpointone"),
        ("get_bpm_list", "GetBPMList", "getbpmlist"),
        ("get_v2_data", "GetV2Data", "getv2data"),
        ("ping", "Ping", "ping"),
        ("read_io", "ReadIO", "readio"),
        ("r#type", "Type", "type"),
        ("get_user_name", "GetUser_Name", "getusername"),
    ] {
        assert_eq!(normalize(rust), normalized, "{rust}");
        assert_eq!(normalize(wire), normalized, "{wire}");
    }
    assert_eq!(normalize("r#type"), normalize("type"));
    assert_ne!(normalize("getEndPointOne"), normalize("getEndPointTwo"));
    assert_ne!(normalize("get"), normalize("getter"));
    assert_eq!(normalize(""), "");
}

#[test]
fn an_inherent_impl_is_rejected() {
    let message = expand_err(
        quote! {},
        quote! {
            impl Server {
                #[public]
                async fn ping(&self, ctx: PublicContext<Req>) -> Out { 1 }
            }
        },
    );

    assert!(message.contains("impl Trait for Type"), "{message}");
}

#[test]
fn a_trait_with_generic_arguments_asks_for_an_explicit_server() {
    let message = expand_err(
        quote! {},
        quote! {
            impl Echo<u8> for Server {
                #[public]
                async fn ping(&self, ctx: PublicContext<Req>) -> Out { 1 }
            }
        },
    );

    assert!(message.contains("without generic arguments"), "{message}");
}
