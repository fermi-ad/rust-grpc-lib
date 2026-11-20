//! Post-processes the code tonic-build generates so that it names `tonic`
//! through `::rust_grpc_lib::__private`, and the consuming crate needs no
//! `tonic` dependency of its own.
//!
//! tonic-build writes every tonic path as `tonic::...`, with no leading `::`
//! and no option to change the prefix, so the paths are rewritten in the
//! generated files, line by line. (The codec and prost paths do have options;
//! see `compile`.) Entry points: [`qualify_tonic_paths`] and
//! [`rewrite_generated_files`].

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

/// Rewrites each path-start `tonic::` in `line` to
/// `::rust_grpc_lib::__private::tonic::`, returning `line` itself if there is
/// none.
///
/// A path starts at `tonic::` when the preceding character is not part of an
/// identifier and is not `:`, `.` or `#`. So these are left alone:
///
/// - `tonic_prost::ProstCodec` and `mytonic::x` (different identifiers),
/// - `::tonic::x` and `a::tonic::x` (already qualified; this also makes the
///   function idempotent),
/// - `x.tonic::<T>()` (a method call) and `r#tonic::x` (a raw identifier).
///
/// Lines whose first non-blank text is `//` (comments, including docs) are
/// returned unchanged. A `tonic::` inside a string literal is rewritten like
/// any other; tonic-build writes no such strings.
pub(super) fn qualify_tonic_paths(line: &str) -> Cow<'_, str> {
    if line.trim_start().starts_with("//") {
        return Cow::Borrowed(line);
    }
    TONIC_PATH_START.replace_all(line, REPLACEMENT)
}

/// Applies [`qualify_tonic_paths`] to the files tonic-build generated in
/// `out_dir` for `packages` (proto package names), plus `google.protobuf.rs`
/// when it exists. No other file in `out_dir` is read or written.
///
/// Fails, naming the file, if a package's file is missing or a file cannot be
/// read or written.
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
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        writer.write_all(qualify_tonic_paths(&line).as_bytes())?;
    }
    writer.flush()?;

    // Must drop so it releases its reference to the tmp file.
    drop(writer);

    fs::rename(temp_path, path)
}

fn describe(path: &Path, error: &io::Error) -> String {
    format!(
        "Error rewriting generated file \"{}\": {error}",
        path.display()
    )
}
