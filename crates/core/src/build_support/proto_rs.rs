//! Generates the `proto.rs` source text from a list of proto package names.
//!
//! The public-to-the-module entry point is [`build_contents`], which wires
//! together the fixed per-package metadata for the Controls proto set and
//! delegates to [`ProtoRsGenerator`].

use std::collections::{BTreeMap, HashMap};

#[cfg(test)]
mod tests;

/// Build the complete `proto.rs` source text for the given sorted package list.
///
/// Applies the fixed per-package metadata required by the Controls proto set
/// (deprecation markers, lint suppression, hidden template modules) and
/// delegates to [`ProtoRsGenerator`].
pub(super) fn build_contents(packages: &[String]) -> String {
    ProtoRsGenerator::new()
        .with_meta("dpm", PackageMeta {
            outer_attrs: vec![
                "#[deprecated = \"Old way of calling DPM. `services::daq` is the replacement in most cases.\"]"
                    .to_string(),
            ],
            ..Default::default()
        })
        .with_meta("services.example", PackageMeta {
            doc: vec![
                "/// The `example` module gets used by the [`grpc-db-template`](https://github.com/fermi-ad/grpc-db-template) repo.".to_string(),
                "/// It is included here for completeness, so the template can use this library out of the gate.".to_string(),
            ],
            ..Default::default()
        })
        .generate(packages)
}

// ---------------------------------------------------------------------------
// PackageMeta
// ---------------------------------------------------------------------------

/// Attributes and doc comments emitted around a generated module.
///
/// Construct one of these for each proto package that needs special treatment
/// (e.g. a `#[deprecated]` outer attribute or an `#![allow(...)]` inner
/// attribute) and register it with [`ProtoRsGenerator::with_meta`].
///
/// [`PackageMeta`] exists to paper over quirks in older proto definitions that
/// cannot be changed. New proto packages should not need it.
#[derive(Debug, Default, PartialEq)]
struct PackageMeta {
    /// Outer attributes placed *before* `pub mod`, e.g. `#[deprecated = "..."]`.
    pub outer_attrs: Vec<String>,
    /// Inner attributes placed *inside* the module body, e.g. `#![allow(...)]`.
    pub inner_attrs: Vec<String>,
    /// Doc comment lines placed before `pub mod`.
    pub doc: Vec<String>,
}

// ---------------------------------------------------------------------------
// ProtoRsGenerator
// ---------------------------------------------------------------------------

/// Generates the `proto.rs` source text from a list of proto package names.
///
/// # Always-present `google.protobuf` module
///
/// Every call to [`generate`](ProtoRsGenerator::generate) unconditionally emits
/// a `pub mod google { pub mod protobuf { tonic::include_proto!("google.protobuf"); } }`
/// block at the top of the output, regardless of the `packages` argument.
/// This module exposes the [well-known types] bundled with `tonic_prost_build`
/// (e.g. `Timestamp`, `Duration`, `Any`) and is required by most tonic-based
/// services.  It is **not** derived from the `interface-definitions/` proto
/// files, so it does not appear in the `packages` list passed to `generate`.
///
/// [well-known types]: https://protobuf.dev/reference/protobuf/google.protobuf/
#[derive(Default)]
struct ProtoRsGenerator {
    meta: HashMap<String, PackageMeta>,
}

impl ProtoRsGenerator {
    fn new() -> Self {
        Self::default()
    }

    /// Register [`PackageMeta`] for a fully-qualified proto package name.
    ///
    /// Returns `self` for method chaining.
    fn with_meta(mut self, package: &str, meta: PackageMeta) -> Self {
        self.meta.insert(package.to_string(), meta);
        self
    }

    /// Generate the complete `proto.rs` source text for the given sorted
    /// package list.
    ///
    /// The output always begins with the `google.protobuf` well-known-types
    /// module, followed by one top-level module per root segment found in
    /// `packages`.
    fn generate(&self, packages: &[String]) -> String {
        let mut out = String::new();

        write_header(&mut out, packages);

        // Emit the google.protobuf well-known types first (not from interface-definitions).
        out.push_str("pub mod google {\n");
        out.push_str("    pub mod protobuf {\n");
        out.push_str("        #![allow(clippy::all)]\n");
        out.push_str("        #![allow(unused)]\n");
        out.push_str("        tonic::include_proto!(\"google.protobuf\");\n");
        out.push_str("    }\n");
        out.push_str("}\n");
        out.push('\n');

        // Emit all other top-level modules from the tree.
        let tree = build_tree(packages);
        for (seg, node) in &tree.children {
            self.emit_node(&mut out, seg, node, 0, "");
            out.push('\n');
        }

        out
    }

    // -----------------------------------------------------------------------
    // Private emit helpers
    // -----------------------------------------------------------------------

    /// Recursively emit a single tree node into `out`.
    fn emit_node(
        &self,
        out: &mut String,
        segment: &str,
        node: &ModNode,
        depth: usize,
        parent_pkg: &str,
    ) {
        let indent = "    ".repeat(depth);
        let mod_name = segment_to_mod_name(segment);

        let this_pkg = if parent_pkg.is_empty() {
            segment.to_string()
        } else {
            format!("{parent_pkg}.{segment}")
        };

        if let Some(pkg) = &node.package {
            self.emit_leaf(out, &indent, &mod_name, pkg);
        } else {
            self.emit_intermediate(out, node, depth, &indent, &mod_name, &this_pkg);
        }
    }

    fn emit_intermediate(
        &self,
        out: &mut String,
        node: &ModNode,
        depth: usize,
        indent: &str,
        mod_name: &str,
        this_pkg: &str,
    ) {
        out.push_str(&format!("{indent}pub mod {mod_name} {{\n"));
        for (child_seg, child_node) in &node.children {
            self.emit_node(out, child_seg, child_node, depth + 1, this_pkg);
        }
        out.push_str(&format!("{indent}}}\n"));
    }

    fn emit_leaf(&self, out: &mut String, indent: &str, mod_name: &str, pkg: &str) {
        let default = PackageMeta::default();
        let m = self.meta.get(pkg).unwrap_or(&default);

        for line in &m.doc {
            out.push_str(&format!("{indent}{line}\n"));
        }
        for attr in &m.outer_attrs {
            out.push_str(&format!("{indent}{attr}\n"));
        }
        out.push_str(&format!("{indent}pub mod {mod_name} {{\n"));
        for attr in &m.inner_attrs {
            out.push_str(&format!("{indent}    {attr}\n"));
        }

        // Now add guards so the included files don't get flagged by the linter.
        // Consumers will not have a way to fix linting errors in the generated code.
        out.push_str(&format!("{indent}    #![allow(clippy::all)]\n"));
        out.push_str(&format!("{indent}    #![allow(unused)]\n"));
        // Prost escapes reserved keywords in generated filenames (e.g. `type` → `r#type`),
        // so the include_proto! argument must use the same escaping.
        let include_arg: String = pkg
            .split('.')
            .map(segment_to_mod_name)
            .collect::<Vec<_>>()
            .join(".");
        out.push_str(&format!(
            "{indent}    tonic::include_proto!(\"{include_arg}\");\n"
        ));
        out.push_str(&format!("{indent}}}\n"));
    }
}

// ---------------------------------------------------------------------------
// File header generator
// ---------------------------------------------------------------------------

fn write_header(out: &mut String, packages: &[String]) {
    out.push_str("// Auto-generated proto.rs file\n");
    out.push_str("//\n");
    out.push_str("// **Contains the rust implementations of the protobuf definitions**\n");
    out.push_str("//\n");
    out.push_str("// -- The following packages are included --\n");
    out.push_str("//\n");
    out.push_str("// Well-known types from Google:\n");
    out.push_str("// * google.protobuf\n");
    out.push_str("//\n");
    out.push_str("// Generated types from interface-definitions:\n");
    for package in packages {
        out.push_str(&format!("// * {package}\n"));
    }
    out.push('\n');
}

// ---------------------------------------------------------------------------
// Module-name helper
// ---------------------------------------------------------------------------

/// All Rust strict keywords and reserved words through edition 2024.
///
/// Any proto package segment that matches one of these must be escaped with
/// `r#` to produce a valid Rust module identifier.
const RUST_KEYWORDS: &[&str] = &[
    /* Strict keywords — all editions: */
    "as", "break", "const", "continue", "crate", "else", "enum", "extern", "false", "fn", "for",
    "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return",
    "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where",
    "while", /* Strict keywords added in edition 2018: */ "async", "await", "dyn",
    /* Strict keyword added in edition 2024: */ "gen",
    /* `union` is a contextual keyword but forbidden as a plain module name */ "union",
    /* Reserved for future use (all editions): */ "abstract", "become", "box", "do", "final",
    "macro", "override", "priv", "try", "typeof", "unsized", "virtual", "yield",
];

/// Convert a proto package segment to a valid Rust module identifier.
///
/// Any segment that matches a keyword in [`RUST_KEYWORDS`] is escaped with the
/// `r#` raw-identifier prefix so the generated code compiles without
/// modification.
fn segment_to_mod_name(segment: &str) -> String {
    if RUST_KEYWORDS.contains(&segment) {
        format!("r#{segment}")
    } else {
        segment.to_string()
    }
}

// ---------------------------------------------------------------------------
// Module tree
// ---------------------------------------------------------------------------

/// A node in the proto-package module tree.
///
/// Leaf nodes carry the full proto package name so we can emit
/// `tonic::include_proto!`.  Intermediate nodes only have children.
///
/// # Assumption
/// No package name may be a proper prefix of another (e.g. `"foo"` and
/// `"foo.bar"` cannot both appear in the same input).  Proto convention makes
/// this safe in practice: intermediate namespace segments (`common`,
/// `services`, …) never carry messages of their own.
#[derive(Default, Debug)]
struct ModNode {
    children: BTreeMap<String, ModNode>,
    /// Set on leaf nodes: the full proto package string (e.g. `"common.alarm"`).
    package: Option<String>,
}

/// Build a [`ModNode`] tree from a slice of fully-qualified proto package names.
///
/// Package names are split on `.`; each segment becomes a tree level.
/// The final segment's node receives `package = Some(full_name)`.
fn build_tree(packages: &[String]) -> ModNode {
    let mut root = ModNode::default();
    for pkg in packages {
        let segments = pkg.split('.').collect::<Vec<_>>();
        let (leaf_seg, ancestor_segs) = segments
            .split_last()
            .expect("package name must not be empty");

        // Walk (or create) every intermediate namespace node.
        let mut node = &mut root;
        for seg in ancestor_segs {
            node = node.children.entry(seg.to_string()).or_default();
            debug_assert!(
                node.package.is_none(),
                "package {:?} is a prefix of {:?}; proto package names must not be proper prefixes of one another",
                node.package.as_deref().unwrap_or(seg),
                pkg,
            );
        }

        // Attach the leaf.
        node = node.children.entry(leaf_seg.to_string()).or_default();
        debug_assert!(
            node.children.is_empty(),
            "package {:?} is a prefix of existing packages {:?}; proto package names must not be proper prefixes of one another",
            pkg,
            node.children.keys().collect::<Vec<_>>(),
        );
        node.package = Some(pkg.clone());
    }
    root
}
