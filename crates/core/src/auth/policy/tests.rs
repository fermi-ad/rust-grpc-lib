//! Tests for the policy table and the method-name normalization.

use super::*;

use crate::testing::claims_with_roles;

struct Impl;
struct ServerA;
struct ServerB;

impl ServicePolicy<ServerA> for Impl {
    const POLICIES: &'static [MethodPolicy] = &[
        MethodPolicy::new("get_end_point_one", Access::any_of(&["operator", "admin"])),
        MethodPolicy::new("get_bpm_list", Access::Authenticated),
        MethodPolicy::new("ping", Access::Public),
    ];
}

// The same type serves a second service whose table differs, including a
// method name shared with the first.
impl ServicePolicy<ServerB> for Impl {
    const POLICIES: &'static [MethodPolicy] = &[MethodPolicy::new(
        "get_bpm_list",
        Access::all_of(&["admin"]),
    )];
}

// The macro's `normalize` (`crates/grpc_macro/src/service/tests.rs`,
// `normalize_matches_what_the_gate_compares`) must agree on these cases; the
// end-to-end check is `crates/integration_tests/src/method_name_matching.rs`.
#[test]
fn method_names_compare_ignoring_case_and_underscores() {
    assert!(same_method("getEndPointOne", "get_end_point_one"));
    assert!(same_method("GetBPMList", "get_bpm_list"));
    assert!(same_method("GetV2Data", "get_v2_data"));
    assert!(same_method("Ping", "ping"));
    assert!(same_method("ReadIO", "read_io"));
    assert!(same_method("Type", "r#type"));
    assert!(same_method("GetUser_Name", "getusername"));
    assert!(same_method("r#type", "type"));
    assert!(!same_method("getEndPointOne", "getEndPointTwo"));
    assert!(!same_method("get", "getter"));
    assert!(!same_method("", "get"));
}

#[test]
fn a_wire_name_finds_the_rust_name() {
    let table = PolicyTable::of::<ServerA, Impl>();

    assert_eq!(
        table.lookup("getEndPointOne"),
        Some(Access::any_of(&["operator", "admin"]))
    );
    assert_eq!(table.lookup("GetBPMList"), Some(Access::Authenticated));
    assert_eq!(table.lookup("ping"), Some(Access::Public));
}

#[test]
fn an_unknown_method_has_no_policy() {
    let table = PolicyTable::of::<ServerA, Impl>();

    assert_eq!(table.lookup("deleteEverything"), None);
    assert_eq!(table.lookup(""), None);
}

#[test]
fn for_service_reads_the_table_the_constructor_names() {
    // Constructors are never called; they only name the types.
    let server_a = |_: Impl| -> ServerA { unreachable!("never called") };
    let server_b = |_: Impl| -> ServerB { unreachable!("never called") };

    let a = PolicyTable::for_service(server_a);
    let b = PolicyTable::for_service(server_b);

    assert_eq!(a.lookup("GetBPMList"), Some(Access::Authenticated));
    assert_eq!(a.lookup("ping"), Some(Access::Public));
    assert_eq!(b.lookup("GetBPMList"), Some(Access::all_of(&["admin"])));
    assert_eq!(b.lookup("ping"), None);
}

#[test]
fn any_of_needs_one_role() {
    let access = Access::any_of(&["operator", "admin"]);

    assert!(access.permits(&claims_with_roles("u", &["admin"])));
    assert!(access.permits(&claims_with_roles("u", &["operator", "admin"])));
    assert!(!access.permits(&claims_with_roles("u", &["viewer"])));
    assert!(!access.permits(&claims_with_roles("u", &[])));
}

#[test]
fn all_of_needs_every_role() {
    let access = Access::all_of(&["operator", "admin"]);

    assert!(access.permits(&claims_with_roles("u", &["operator", "admin", "x"])));
    assert!(!access.permits(&claims_with_roles("u", &["admin"])));
    assert!(!access.permits(&claims_with_roles("u", &[])));
}

#[test]
#[should_panic(expected = "at least one role")]
fn an_empty_any_of_is_refused() {
    let _ = Access::any_of(&[]);
}

#[test]
#[should_panic(expected = "at least one role")]
fn an_empty_all_of_is_refused() {
    let _ = Access::all_of(&[]);
}
