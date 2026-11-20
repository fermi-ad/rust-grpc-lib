//! Rewrites tonic-build output to name `tonic` through
//! `::rust_grpc_lib::__private`, because tonic-build has no option to change
//! that prefix. The pass also adds wire method metadata.

use std::{
    borrow::Cow,
    fs::{self, File},
    io::{self, BufRead, BufReader, BufWriter, Write},
    path::Path,
    sync::LazyLock,
};

use regex::Regex;

use super::proto_rs::package_to_file_stem;

#[cfg(test)]
mod tests;

// A `tonic::` that starts a path. Group 1 is the line start or the character
// before `tonic`, which must not continue an identifier (`my_tonic::`; `\w` is
// Unicode-aware, so `étonic::` counts too; `tonic_prost::` never matches since
// `::` must follow `tonic` directly) or a path (`::tonic::`, `a::tonic::`), and
// must not make it a method call (`x.tonic::<T>()`) or a raw identifier
// (`r#tonic::`). The regex crate has no look-behind, so that character is
// matched and put back.
static TONIC_PATH_START: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(^|[^\w:.#])tonic::").expect("the tonic path pattern is valid"));

// Where `tonic` lives from a consumer's point of view: `rust-grpc-lib`
// re-exports it under `__private`.
const REPLACEMENT: &str = "${1}::rust_grpc_lib::__private::tonic::";
const NAMED_SERVICE_IMPL: &str =
    "    impl<T> ::rust_grpc_lib::__private::tonic::server::NamedService for ";
const WIRE_METHODS_IMPL: &str = "    impl<T> ::rust_grpc_lib::__private::WireMethods for ";

/// Qualifies each path-start `tonic::`; already-qualified paths, other
/// identifiers, method calls and raw identifiers are left alone, so it is
/// idempotent. Comment lines are unchanged; a `tonic::` inside a string
/// literal is rewritten (tonic-build writes none).
pub(super) fn qualify_tonic_paths(line: &str) -> Cow<'_, str> {
    if line.trim_start().starts_with("//") {
        return Cow::Borrowed(line);
    }
    TONIC_PATH_START.replace_all(line, REPLACEMENT)
}

/// Rewrites only the files for `packages` and `google.protobuf.rs` if present.
/// Errors name the offending file.
pub(super) fn rewrite_generated_files(out_dir: &Path, packages: &[String]) -> Result<(), String> {
    for package in packages {
        let path = out_dir.join(format!("{}.rs", package_to_file_stem(package)));
        rewrite_file(&path).map_err(|e| describe(&path, &e))?;
    }

    // Written only when some bundled proto imports a well-known type, so it
    // may be absent. Treating it like every other generated file keeps the
    // contract simple.
    let well_known = out_dir.join("google.protobuf.rs");
    if well_known.exists() {
        rewrite_file(&well_known).map_err(|e| describe(&well_known, &e))?;
    }
    Ok(())
}

// Rewrites one file through a temporary sibling file, which then replaces the
// original, so a failure part-way leaves the original intact. Lines keep their
// original line endings.
fn rewrite_file(path: &Path) -> io::Result<()> {
    let mut reader = BufReader::new(File::open(path)?);

    let mut temp_name = path.as_os_str().to_owned();
    temp_name.push(".tmp");
    let temp_path = Path::new(&temp_name);
    let mut writer = BufWriter::new(File::create(temp_path)?);

    let mut line = String::new();
    let mut server_wire_methods = None;
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let qualified = qualify_tonic_paths(&line);
        let text = qualified.trim_end_matches(['\r', '\n']);
        // tonic-build emits unindented module boundaries. On a repeat pass,
        // our WireMethods impl appears before the NamedService anchor.
        if text.starts_with("pub mod ") {
            server_wire_methods = text.ends_with("_server {").then(Vec::new);
        } else if text == "}" || text.starts_with(WIRE_METHODS_IMPL) {
            server_wire_methods = None;
        } else if let Some(wire_methods) = &mut server_wire_methods {
            if let Some(wire_method) = text
                .trim_start()
                .strip_prefix("\"/")
                .and_then(|arm| arm.strip_suffix("\" => {"))
            {
                wire_methods.push(wire_method.to_owned());
            } else if let Some(server_type) = text
                .strip_prefix(NAMED_SERVICE_IMPL)
                .and_then(|tail| tail.strip_suffix("<T> {"))
            {
                write_wire_methods_impl(
                    &mut writer,
                    server_type,
                    wire_methods,
                    &qualified[text.len()..],
                )?;
                server_wire_methods = None;
            }
        }
        writer.write_all(qualified.as_bytes())?;
    }
    writer.flush()?;

    // Must drop so it releases its reference to the tmp file.
    drop(writer);

    fs::rename(temp_path, path)
}

fn write_wire_methods_impl(
    writer: &mut impl Write,
    server_type: &str,
    wire_methods: &[String],
    line_ending: &str,
) -> io::Result<()> {
    write!(
        writer,
        "{WIRE_METHODS_IMPL}{server_type}<T> {{{line_ending}"
    )?;
    write!(
        writer,
        "        const WIRE_METHODS: &'static [&'static str] = &["
    )?;
    if !wire_methods.is_empty() {
        write!(writer, "{line_ending}")?;
        for wire_method in wire_methods {
            write!(writer, "            \"{wire_method}\",{line_ending}")?;
        }
        write!(writer, "        ")?;
    }
    write!(writer, "];{line_ending}    }}{line_ending}")
}

fn describe(path: &Path, error: &io::Error) -> String {
    format!(
        "Error rewriting generated file \"{}\": {error}",
        path.display()
    )
}
