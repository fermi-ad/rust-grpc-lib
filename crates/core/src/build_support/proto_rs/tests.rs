//! Tests for [`super::segment_to_mod_name`], [`super::package_to_file_stem`],
//! [`super::build_tree`], and [`super::ProtoRsGenerator::generate`].
//!
//! Covers: keyword escaping (checked against real prost-build output), tree
//! construction from package lists (including shared prefixes and sort order),
//! invariant violations (debug-only panics), and the full `generate` output
//! for various package configurations.

use std::{env, fs, path::PathBuf};

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
fn segment_to_mod_name_all_raw_prefix_words_escaped() {
    // Every entry in RAW_PREFIX_WORDS must become a raw identifier,
    // including `gen` which became a strict keyword in edition 2024.
    for kw in RAW_PREFIX_WORDS {
        assert_eq!(segment_to_mod_name(kw), format!("r#{kw}"));
    }
}

#[test]
fn segment_to_mod_name_suffix_words_get_underscore() {
    // These cannot be raw identifiers, so prost appends `_`.
    for kw in SUFFIX_WORDS {
        assert_eq!(segment_to_mod_name(kw), format!("{kw}_"));
    }
    assert_eq!(segment_to_mod_name("self"), "self_");
    assert_eq!(segment_to_mod_name("super"), "super_");
    assert_eq!(segment_to_mod_name("crate"), "crate_");
    assert_eq!(segment_to_mod_name("extern"), "extern_");
    assert_eq!(segment_to_mod_name("Self"), "Self_");
}

#[test]
fn segment_to_mod_name_union_is_left_plain() {
    // `union` is only a contextual keyword, and prost does not escape it.
    assert_eq!(segment_to_mod_name("union"), "union");
}

#[test]
fn segment_to_mod_name_lists_are_disjoint() {
    for kw in SUFFIX_WORDS {
        assert!(
            !RAW_PREFIX_WORDS.contains(kw),
            "`{kw}` is in both escaping lists"
        );
    }
}

#[test]
fn package_to_file_stem_escapes_each_segment() {
    // Module names and file names come from the same per-segment escaping.
    assert_eq!(package_to_file_stem("common.alarm"), "common.alarm");
    assert_eq!(
        package_to_file_stem("third_party.type"),
        "third_party.r#type"
    );
    assert_eq!(package_to_file_stem("demo.self.v1"), "demo.self_.v1");
    assert_eq!(package_to_file_stem("demo.union"), "demo.union");
}

// -----------------------------------------------------------------------
// file names against real prost-build
// -----------------------------------------------------------------------

// Runs the vendored `protoc` and prost-build over one tiny proto per package
// and returns the sorted names of the files written to the scratch `OUT_DIR`.
fn prost_file_names(name: &str, packages: &[&str]) -> Vec<String> {
    let dir: PathBuf = env::temp_dir().join(format!(
        "rust-grpc-lib-proto-rs-{name}-{}",
        std::process::id()
    ));
    let out = dir.join("out");
    fs::create_dir_all(&out).expect("create scratch dir");

    let protos: Vec<PathBuf> = packages
        .iter()
        .enumerate()
        .map(|(i, package)| {
            let path = dir.join(format!("p{i}.proto"));
            let source = format!("syntax = \"proto3\";\npackage {package};\n\nmessage Ping {{}}\n");
            fs::write(&path, source).expect("write proto");
            path
        })
        .collect();

    // SAFETY: tests that touch the environment all set the same value.
    unsafe {
        env::set_var(
            "PROTOC",
            protoc_bin_vendored::protoc_bin_path().expect("vendored protoc"),
        );
    }
    tonic_prost_build::configure()
        .out_dir(&out)
        .compile_protos(&protos, std::slice::from_ref(&dir))
        .expect("protoc and tonic-prost-build succeed");

    let mut names: Vec<String> = fs::read_dir(&out)
        .expect("read OUT_DIR")
        .map(|entry| {
            entry
                .expect("dir entry")
                .file_name()
                .into_string()
                .expect("utf-8 file name")
        })
        .collect();
    names.sort();
    let _ = fs::remove_dir_all(&dir);
    names
}

#[test]
fn package_to_file_stem_matches_prost_build_file_names() {
    // One package per escaping class: `r#` words, `_` words, `union` (plain),
    // and an ordinary name. Each is a leaf, so none is a prefix of another.
    let packages = [
        "demo.self.v1",
        "demo.super",
        "demo.crate",
        "demo.extern",
        "demo.type",
        "demo.async",
        "demo.union",
        "demo.plain",
    ];

    let mut expected: Vec<String> = packages
        .iter()
        .map(|package| format!("{}.rs", package_to_file_stem(package)))
        .collect();
    expected.sort();

    assert_eq!(prost_file_names("file-names", &packages), expected);
}

// -----------------------------------------------------------------------
// build_tree
// -----------------------------------------------------------------------

#[test]
fn build_tree_single_segment_package() {
    let pkgs = vec!["dpm".to_string()];
    let tree = build_tree(&pkgs);
    assert!(tree.children.contains_key("dpm"));
    assert_eq!(tree.children["dpm"].package, Some("dpm".to_string()));
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
    assert!(out.contains("include!(concat!(env!(\"OUT_DIR\"), \"/some.r#type.rs\"))"));
}

#[test]
fn generate_suffix_and_plain_keyword_segments() {
    let out = plain().generate(&["demo.self.v1".to_string(), "demo.union".to_string()]);
    assert!(out.contains("pub mod self_ {"));
    assert!(out.contains("include!(concat!(env!(\"OUT_DIR\"), \"/demo.self_.v1.rs\"))"));
    assert!(out.contains("pub mod union {"));
    assert!(out.contains("include!(concat!(env!(\"OUT_DIR\"), \"/demo.union.rs\"))"));
    assert!(!out.contains("r#self"));
    assert!(!out.contains("r#union"));
}

// -----------------------------------------------------------------------
// generate (top-level behaviour)
// -----------------------------------------------------------------------

#[test]
fn generate_always_includes_google_protobuf() {
    let out = plain().generate(&[]);
    assert!(out.contains("pub mod google {"));
    assert!(out.contains("pub mod protobuf {"));
    assert!(out.contains("include!(concat!(env!(\"OUT_DIR\"), \"/google.protobuf.rs\"))"));
}

#[test]
fn generate_single_package() {
    let pkgs = vec!["common.alarm".to_string()];
    let out = plain().generate(&pkgs);
    assert!(out.contains("pub mod common {"));
    assert!(out.contains("pub mod alarm {"));
    assert!(out.contains("include!(concat!(env!(\"OUT_DIR\"), \"/common.alarm.rs\"))"));
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
