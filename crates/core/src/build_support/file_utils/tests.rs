//! Tests for `file_utils` — covers proto package extraction from file contents.

use super::*;

// -----------------------------------------------------------------------
// extract_package
// -----------------------------------------------------------------------

#[test]
fn extract_package_simple() {
    let proto = "syntax = \"proto3\";\npackage foo.bar;\n";
    assert_eq!(extract_package(proto), Some("foo.bar".to_string()));
}

#[test]
fn extract_package_with_leading_whitespace() {
    let proto = "  package   my.pkg  ;\n";
    assert_eq!(extract_package(proto), Some("my.pkg".to_string()));
}

#[test]
fn extract_package_no_package_line() {
    let proto = "syntax = \"proto3\";\nmessage Foo {}\n";
    assert_eq!(extract_package(proto), None);
}

#[test]
fn extract_package_empty_package_name() {
    // "package ;" — the name after stripping the semicolon is empty.
    let proto = "package ;\n";
    assert_eq!(extract_package(proto), None);
}

#[test]
fn extract_package_returns_first_occurrence() {
    // Malformed file with two package lines — we return the first.
    let proto = "package first;\npackage second;\n";
    assert_eq!(extract_package(proto), Some("first".to_string()));
}
