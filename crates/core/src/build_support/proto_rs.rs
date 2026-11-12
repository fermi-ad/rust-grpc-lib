//! Renders `proto.rs`: a `pub mod` tree mirroring the proto package names,
//! whose leaves `include!` the per-package files from `OUT_DIR` (the expansion
//! of `tonic::include_proto!`, written out so the consumer needs no `tonic`
//! dependency). Entry point: [`build_contents`].

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write;

#[cfg(test)]
mod tests;

/// Returns the `proto.rs` source for a sorted package list, with the
/// per-package extras the bundled protos need: `dpm` is deprecated and
/// `services.example` is documented as the `grpc-db-template` module.
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

// Renders `proto.rs`, adding the registered `PackageMeta` around leaf modules.
#[derive(Default)]
struct ProtoRsGenerator {
    meta: HashMap<String, PackageMeta>,
}

impl ProtoRsGenerator {
    fn new() -> Self {
        Self::default()
    }

    // Registers `meta` for the exact proto package name `package`.
    fn with_meta(mut self, package: &str, meta: PackageMeta) -> Self {
        self.meta.insert(package.to_string(), meta);
        self
    }

    // Renders the header comment, `google::protobuf`, then one top-level
    // module per root segment of `packages`.
    fn generate(&self, packages: &[String]) -> String {
        let mut out = String::new();

        write_header(&mut out, packages);

        // Always emitted: `Config::new` turns on `compile_well_known_types`,
        // and these files are imported by the bundled protos rather than
        // bundled, so they never appear in `packages`.
        out.push_str("pub mod google {\n");
        out.push_str("    pub mod protobuf {\n");
        out.push_str("        #![allow(clippy::all, clippy::pedantic)]\n");
        out.push_str("        #![allow(unused)]\n");
        out.push_str("        include!(concat!(env!(\"OUT_DIR\"), \"/google.protobuf.rs\"));\n");
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

    // Emits one tree node, recursing into its children.
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

    // Emits a namespace module that only wraps its child modules.
    fn emit_intermediate(
        &self,
        out: &mut String,
        node: &ModNode,
        depth: usize,
        indent: &str,
        mod_name: &str,
        this_pkg: &str,
    ) {
        writeln!(out, "{indent}pub mod {mod_name} {{").expect("writing to a String cannot fail");
        for (child_seg, child_node) in &node.children {
            self.emit_node(out, child_seg, child_node, depth + 1, this_pkg);
        }
        writeln!(out, "{indent}}}").expect("writing to a String cannot fail");
    }

    // Emits a module that includes the code generated for one proto package.
    fn emit_leaf(&self, out: &mut String, indent: &str, mod_name: &str, pkg: &str) {
        let default = PackageMeta::default();
        let m = self.meta.get(pkg).unwrap_or(&default);

        for line in &m.doc {
            writeln!(out, "{indent}{line}").expect("writing to a String cannot fail");
        }
        for attr in &m.outer_attrs {
            writeln!(out, "{indent}{attr}").expect("writing to a String cannot fail");
        }
        writeln!(out, "{indent}pub mod {mod_name} {{").expect("writing to a String cannot fail");
        for attr in &m.inner_attrs {
            writeln!(out, "{indent}    {attr}").expect("writing to a String cannot fail");
        }

        // Lint guards: consumers cannot fix warnings in generated code.
        writeln!(out, "{indent}    #![allow(clippy::all, clippy::pedantic)]")
            .expect("writing to a String cannot fail");
        writeln!(out, "{indent}    #![allow(unused)]").expect("writing to a String cannot fail");
        let include_arg = package_to_file_stem(pkg);
        writeln!(
            out,
            "{indent}    include!(concat!(env!(\"OUT_DIR\"), \"/{include_arg}.rs\"));"
        )
        .expect("writing to a String cannot fail");
        writeln!(out, "{indent}}}").expect("writing to a String cannot fail");
    }
}

// Extra lines emitted around one package's leaf module. Each entry is a full
// line of Rust source and is emitted verbatim.
#[derive(Debug, Default, PartialEq)]
struct PackageMeta {
    // Placed before `pub mod`, e.g. `#[deprecated = "..."]`.
    outer_attrs: Vec<String>,
    // Placed inside the module body, e.g. `#![allow(...)]`.
    inner_attrs: Vec<String>,
    // Doc comment lines placed before `pub mod`.
    doc: Vec<String>,
}

// A node in the package module tree: a leaf (with the full package name for
// `include!`) or a namespace (with children), never both. So no package
// may be a proper prefix of another (`foo` and `foo.bar`); `build_tree` checks
// this with `debug_assert!` only, and in release the leaf's children would be
// silently dropped by `emit_node`.
#[derive(Default, Debug)]
struct ModNode {
    // Sorted, so the output is deterministic.
    children: BTreeMap<String, ModNode>,
    // Set on leaf nodes: the full package name, e.g. `"common.alarm"`.
    package: Option<String>,
}

// Splits each package name on `.` into tree levels; the last segment's node
// gets `package = Some(full_name)`.
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

// Writes the leading comment block of `proto.rs`, which lists the packages.
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
        writeln!(out, "// * {package}").expect("writing to a String cannot fail");
    }
    out.push('\n');
}

// The two lists below mirror `sanitize_identifier` in prost-build 0.14.4
// (`src/ident.rs`), which `Module::from_protobuf_package_name` applies to every
// package segment. Prost uses that one result for both the `mod` name and the
// generated file name (`Module::to_file_name_or`), so this file does too. Keep
// them in sync when `prost-build` is upgraded.
//
// Not mirrored: prost snake-cases a segment before escaping it (`Type` becomes
// `r#type`), and prefixes a leading digit with `_`. Proto package segments in
// this repo are already lower snake case and cannot start with a digit.

// Words prost writes as raw identifiers (`r#type`): Rust strict and reserved
// keywords that are valid as raw identifiers, through edition 2024. `union` is
// only a contextual keyword, so prost leaves it plain.
const RAW_PREFIX_WORDS: &[&str] = &[
    /* Strict keywords — all editions: */
    "as", "break", "const", "continue", "else", "enum", "false", "fn", "for", "if", "impl", "in",
    "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return", "static", "struct",
    "trait", "true", "type", "unsafe", "use", "where", "while",
    /* Strict keywords added in edition 2018: */ "async", "await", "dyn",
    /* Strict keyword added in edition 2024: */ "gen",
    /* Reserved for future use (all editions): */ "abstract", "become", "box", "do", "final",
    "macro", "override", "priv", "try", "typeof", "unsized", "virtual", "yield",
];

// Keywords that cannot be raw identifiers; prost appends `_` (`self_`).
const SUFFIX_WORDS: &[&str] = &["_", "crate", "extern", "self", "Self", "super"];

// Returns `segment` as prost-build names it, in both the module path and the
// generated file name: `r#` for `RAW_PREFIX_WORDS`, a trailing `_` for
// `SUFFIX_WORDS`, otherwise unchanged.
fn segment_to_mod_name(segment: &str) -> String {
    if RAW_PREFIX_WORDS.contains(&segment) {
        format!("r#{segment}")
    } else if SUFFIX_WORDS.contains(&segment) {
        format!("{segment}_")
    } else {
        segment.to_string()
    }
}

// Returns the name prost-build gives the file for `package`, without `.rs`;
// `include!` names the file `<stem>.rs` in `OUT_DIR`.
pub(super) fn package_to_file_stem(package: &str) -> String {
    package
        .split('.')
        .map(segment_to_mod_name)
        .collect::<Vec<_>>()
        .join(".")
}
