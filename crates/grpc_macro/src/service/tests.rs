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

// Pins `normalize` against hard-coded strings. That `normalize` agrees with
// the gate is checked by `normalize_agrees_with_the_gates_lookup` below.
// Mirrors `method_names_compare_ignoring_case_and_underscores` in
// `crates/core/src/auth/policy/tests.rs` and the end-to-end
// `crates/integration_tests/src/method_name_matching.rs`.
#[test]
fn normalize_maps_rust_and_wire_spellings_together() {
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
fn a_plain_collision_does_not_mention_cfg() {
    let message = expand_err(
        quote! {},
        quote! {
            impl Echo for Server {
                #[public]
                async fn get_user(&self, ctx: PublicContext<Req>) -> Out { 1 }

                #[public]
                async fn getuser(&self, ctx: PublicContext<Req>) -> Out { 2 }
            }
        },
    );

    assert!(message.contains("collides with `get_user`"), "{message}");
    assert!(!message.contains("cfg"), "{message}");
}

#[test]
fn a_collision_between_cfg_gated_methods_explains_how_to_restructure() {
    let message = expand_err(
        quote! {},
        quote! {
            impl Echo for Server {
                #[cfg(feature = "a")]
                #[public]
                async fn ping(&self, ctx: PublicContext<Req>) -> Out { 1 }

                #[cfg(not(feature = "a"))]
                #[any_authenticated]
                async fn ping(&self, ctx: RequestContext<Req>) -> Out { 2 }
            }
        },
    );

    assert!(message.contains("collides with `ping`"), "{message}");
    assert!(
        message.contains("one `impl` block per variant"),
        "{message}"
    );
}

#[test]
fn a_collision_with_only_the_first_method_gated_also_gets_the_hint() {
    let message = expand_err(
        quote! {},
        quote! {
            impl Echo for Server {
                #[cfg_attr(test, allow(unused))]
                #[public]
                async fn ping(&self, ctx: PublicContext<Req>) -> Out { 1 }

                #[public]
                async fn Ping(&self, ctx: PublicContext<Req>) -> Out { 2 }
            }
        },
    );

    assert!(
        message.contains("one `impl` block per variant"),
        "{message}"
    );
}

// A policy table like the one the macro emits, with a distinct role per row
// so a lookup identifies which row it found. It is read by core's real
// `PolicyTable::lookup`, the matcher the gate uses, not a copy of it.
struct Stub;
struct StubServer;

impl rust_grpc_lib::auth::ServicePolicy<StubServer> for Stub {
    const POLICIES: &'static [rust_grpc_lib::auth::MethodPolicy] = {
        use rust_grpc_lib::auth::{Access, MethodPolicy};
        &[
            MethodPolicy::new("get_end_point_one", Access::any_of(&["one"])),
            MethodPolicy::new("get_bpm_list", Access::any_of(&["two"])),
            MethodPolicy::new("get_v2_data", Access::any_of(&["three"])),
            MethodPolicy::new("ping", Access::any_of(&["four"])),
            MethodPolicy::new("read_io", Access::any_of(&["five"])),
            // The macro emits `r#type` as `type`.
            MethodPolicy::new("type", Access::any_of(&["six"])),
            MethodPolicy::new("get_user_name", Access::any_of(&["seven"])),
        ]
    };
}

// If the gate finds a row for a wire name, `normalize` must give that wire
// name and the row's Rust name the same string, and the reverse: otherwise
// `check_collisions` could pass two methods the gate cannot tell apart.
#[test]
fn normalize_agrees_with_the_gates_lookup() {
    let table = rust_grpc_lib::auth::PolicyTable::for_service(|_: Stub| StubServer);
    let row = |rust: &str| {
        let rust = rust.trim_start_matches("r#");
        table
            .policies()
            .find(|policy| policy.method == rust)
            .map(|policy| policy.access)
    };

    for (rust, wire) in [
        ("get_end_point_one", "getEndPointOne"),
        ("get_bpm_list", "GetBPMList"),
        ("get_v2_data", "GetV2Data"),
        ("ping", "Ping"),
        ("read_io", "ReadIO"),
        ("r#type", "Type"),
        ("get_user_name", "GetUser_Name"),
    ] {
        let expected = row(rust);
        assert!(expected.is_some(), "no row for {rust}");
        assert_eq!(table.lookup(wire), expected, "{wire}");
        assert_eq!(table.lookup(rust), expected, "{rust}");
        assert_eq!(normalize(wire), normalize(rust), "{wire} vs {rust}");
    }

    // Names the gate does not match must not normalize to a row's name.
    for other in [
        "getEndPointThree",
        "get",
        "getter",
        "pin",
        "pings",
        "GetUserNames",
        "",
    ] {
        assert_eq!(table.lookup(other), None, "{other}");
        for policy in table.policies() {
            assert_ne!(normalize(other), normalize(policy.method), "{other}");
        }
    }
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
fn an_explicit_server_with_generic_arguments_is_rejected() {
    let message = expand_err(
        quote! { server = other::CustomServer<Server> },
        quote! {
            impl Echo for Server {
                #[public]
                async fn ping(&self, ctx: PublicContext<Req>) -> Out { 1 }
            }
        },
    );

    assert!(message.contains("without generic arguments"), "{message}");
}

// With `server = ..` the trait path is never turned into a server name, so
// generic arguments on it are fine (documented in `lib.rs`).
#[test]
fn an_explicit_server_skips_the_trait_path_generics_check() {
    let out = expand_ok(
        quote! { server = other::CustomServer },
        quote! {
            impl Echo<u8> for Server {
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

#[test]
fn a_macro_invocation_among_the_items_is_rejected() {
    let message = expand_err(
        quote! {},
        quote! {
            impl Echo for Server {
                #[public]
                async fn ping(&self, ctx: PublicContext<Req>) -> Out { 1 }

                generate_handlers!();
            }
        },
    );

    assert!(message.contains("cannot see"), "{message}");
}

#[test]
fn associated_types_and_consts_pass_through() {
    let out = expand_ok(
        quote! {},
        quote! {
            impl Echo for Server {
                type Stream = S;
                const LIMIT: u8 = 1;

                #[public]
                async fn ping(&self, ctx: PublicContext<Req>) -> Out { 1 }
            }
        },
    )
    .replace(' ', "");

    assert!(out.contains("typeStream=S;constLIMIT:u8=1;"), "{out}");
}
