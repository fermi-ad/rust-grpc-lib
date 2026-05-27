use super::*;

fn plain() -> ProtoRsGenerator {
    ProtoRsGenerator::new()
}

// -----------------------------------------------------------------------
// segment_to_mod_name
// -----------------------------------------------------------------------

#[test]
fn segment_to_mod_name_ordinary() {
    assert_eq!(segment_to_mod_name("common"), "common");
    assert_eq!(segment_to_mod_name("services"), "services");
    assert_eq!(segment_to_mod_name("alarm"), "alarm");
}

#[test]
fn segment_to_mod_name_reserved_type() {
    assert_eq!(segment_to_mod_name("type"), "r#type");
}

#[test]
fn segment_to_mod_name_all_keywords_escaped() {
    // Every entry in the authoritative RUST_KEYWORDS list must be escaped,
    // including `gen` which became a strict keyword in edition 2024.
    for kw in RUST_KEYWORDS {
        let result = segment_to_mod_name(kw);
        assert!(
            result.starts_with("r#"),
            "keyword `{kw}` was not escaped: got `{result}`"
        );
    }
}

#[test]
fn segment_to_mod_name_gen_escaped() {
    // Explicit spot-check for the edition-2024 addition.
    assert_eq!(segment_to_mod_name("gen"), "r#gen");
}

// -----------------------------------------------------------------------
// build_tree
// -----------------------------------------------------------------------

#[test]
fn build_tree_empty_input() {
    let tree = build_tree(&[]);
    assert!(tree.children.is_empty());
    assert!(tree.package.is_none());
}

#[test]
fn build_tree_single_segment_package() {
    let pkgs = vec!["dpm".to_string()];
    let tree = build_tree(&pkgs);
    assert!(tree.children.contains_key("dpm"));
    assert_eq!(tree.children["dpm"].package, Some("dpm".to_string()));
}

#[test]
fn build_tree_two_segment_package() {
    let pkgs = vec!["common.alarm".to_string()];
    let tree = build_tree(&pkgs);
    let common = &tree.children["common"];
    assert!(
        common.package.is_none(),
        "intermediate node should have no package"
    );
    let alarm = &common.children["alarm"];
    assert_eq!(alarm.package, Some("common.alarm".to_string()));
}

#[test]
fn build_tree_shared_prefix() {
    let pkgs = vec!["common.alarm".to_string(), "common.event".to_string()];
    let tree = build_tree(&pkgs);
    let common = &tree.children["common"];
    assert!(common.package.is_none());
    assert!(common.children.contains_key("alarm"));
    assert!(common.children.contains_key("event"));
}

#[test]
fn build_tree_children_are_sorted() {
    let pkgs = vec![
        "z.last".to_string(),
        "a.first".to_string(),
        "m.middle".to_string(),
    ];
    let tree = build_tree(&pkgs);
    let keys: Vec<&String> = tree.children.keys().collect();
    assert_eq!(keys, vec!["a", "m", "z"]);
}

// -----------------------------------------------------------------------
// build_tree — invariant violations (debug_assert)
// -----------------------------------------------------------------------

/// Violation 1: a short package (`"foo"`) is already a leaf, then a longer
/// package (`"foo.bar"`) arrives and tries to walk through it as a namespace.
#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "is a prefix of")]
fn build_tree_leaf_used_as_namespace_panics() {
    let pkgs = vec!["foo".to_string(), "foo.bar".to_string()];
    build_tree(&pkgs);
}

/// Violation 2: a longer package (`"foo.bar"`) is already in the tree, then
/// a shorter package (`"foo"`) arrives and tries to register the namespace
/// node as a leaf.
#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "is a prefix of")]
fn build_tree_namespace_registered_as_leaf_panics() {
    let pkgs = vec!["foo.bar".to_string(), "foo".to_string()];
    build_tree(&pkgs);
}

// -----------------------------------------------------------------------
// emit helpers (via generate)
// -----------------------------------------------------------------------

#[test]
fn generate_plain_package_no_attrs() {
    let out = plain().generate(&["common.alarm".to_string()]);
    assert!(out.contains("pub mod alarm {"));
    assert!(out.contains("tonic::include_proto!(\"common.alarm\")"));
    assert!(!out.contains("#[deprecated"));
}

#[test]
fn generate_with_outer_attr() {
    let out = ProtoRsGenerator::new()
        .with_meta(
            "dpm",
            PackageMeta {
                outer_attrs: vec!["#[deprecated = \"use daq\"]".to_string()],
                ..Default::default()
            },
        )
        .generate(&["dpm".to_string()]);
    assert!(out.contains("#[deprecated"));
    assert!(out.contains("pub mod dpm {"));
}

#[test]
fn generate_with_inner_attr() {
    let out = ProtoRsGenerator::new()
        .with_meta(
            "common.event",
            PackageMeta {
                inner_attrs: vec!["#![allow(clippy::module_inception)]".to_string()],
                ..Default::default()
            },
        )
        .generate(&["common.event".to_string()]);
    assert!(out.contains("#![allow(clippy::module_inception)]"));
}

#[test]
fn generate_type_segment_uses_raw_identifier() {
    let out = plain().generate(&["some.type".to_string()]);
    assert!(out.contains("pub mod r#type {"));
    assert!(out.contains("tonic::include_proto!(\"some.r#type\")"));
}

#[test]
fn generate_intermediate_wraps_children() {
    let pkgs = vec!["common.alarm".to_string(), "common.event".to_string()];
    let out = plain().generate(&pkgs);
    assert!(out.contains("pub mod common {"));
    assert!(out.contains("pub mod alarm {"));
    assert!(out.contains("pub mod event {"));
}

// -----------------------------------------------------------------------
// generate (top-level behaviour)
// -----------------------------------------------------------------------

#[test]
fn generate_always_includes_google_protobuf() {
    let out = plain().generate(&[]);
    assert!(out.contains("pub mod google {"));
    assert!(out.contains("pub mod protobuf {"));
    assert!(out.contains("tonic::include_proto!(\"google.protobuf\")"));
}

#[test]
fn generate_empty_packages() {
    let out = plain().generate(&[]);
    assert!(
            out.trim_end().ends_with("pub mod google {\n    pub mod protobuf {\n        #![allow(clippy::all)]\n        #![allow(unused)]\n        tonic::include_proto!(\"google.protobuf\");\n    }\n}")
        );
}

#[test]
fn generate_single_package() {
    let pkgs = vec!["common.alarm".to_string()];
    let out = plain().generate(&pkgs);
    assert!(out.contains("pub mod common {"));
    assert!(out.contains("pub mod alarm {"));
    assert!(out.contains("tonic::include_proto!(\"common.alarm\")"));
}

#[test]
fn generate_multiple_packages_sorted() {
    let pkgs = vec![
        "services.daq".to_string(),
        "common.alarm".to_string(),
        "dpm".to_string(),
    ];
    let out = plain().generate(&pkgs);
    let pos_common = out.find("pub mod common").unwrap();
    let pos_dpm = out.find("pub mod dpm").unwrap();
    let pos_services = out.find("pub mod services").unwrap();
    assert!(pos_common < pos_dpm);
    assert!(pos_dpm < pos_services);
}

#[test]
fn generate_no_meta_emits_no_extra_attrs() {
    let out = plain().generate(&["dpm".to_string()]);
    assert!(!out.contains("#[deprecated"));
}

#[test]
fn generate_well_known_types_come_first() {
    let pkgs = vec!["aaa.first".to_string()];
    let out = plain().generate(&pkgs);
    let pos_google = out.find("pub mod google").unwrap();
    let pos_aaa = out.find("pub mod aaa").unwrap();
    assert!(
        pos_google < pos_aaa,
        "google block must precede other modules"
    );
}

#[test]
fn generate_full_snapshot() {
    let mut pkgs = vec![
        "common.alarm".to_string(),
        "common.event".to_string(),
        "dpm".to_string(),
        "services.devdb".to_string(),
        "services.example".to_string(),
    ];
    pkgs.sort();

    let out = ProtoRsGenerator::new()
        .with_meta(
            "dpm",
            PackageMeta {
                outer_attrs: vec!["#[deprecated = \"use daq\"]".to_string()],
                ..Default::default()
            },
        )
        .with_meta(
            "common.event",
            PackageMeta {
                inner_attrs: vec!["#![allow(clippy::module_inception)]".to_string()],
                ..Default::default()
            },
        )
        .with_meta(
            "services.devdb",
            PackageMeta {
                inner_attrs: vec!["#![allow(clippy::large_enum_variant)]".to_string()],
                ..Default::default()
            },
        )
        .with_meta(
            "services.example",
            PackageMeta {
                outer_attrs: vec!["#[doc(hidden)]".to_string()],
                doc: vec!["/// Example module.".to_string()],
                ..Default::default()
            },
        )
        .generate(&pkgs);

    assert!(out.contains("pub mod google {"));
    assert!(out.contains("pub mod common {"));
    assert!(out.contains("#[deprecated"));
    assert!(out.contains("#![allow(clippy::large_enum_variant)]"));
    assert!(out.contains("#[doc(hidden)]"));
    assert!(out.contains("#![allow(clippy::module_inception)]"));
}
