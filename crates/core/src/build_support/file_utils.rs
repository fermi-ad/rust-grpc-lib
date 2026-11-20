//! Finds the bundled protos and reads their `package` names for
//! [`generate_protos`](super::generate_protos). Errors are `String`s, which
//! `?` turns into the caller's `Box<dyn Error>`.

use std::{
    ffi::OsStr,
    fs::{self, read_dir},
};

#[cfg(test)]
mod tests;

/// Returns every file under `dir` (recursively) with extension `proto_ext`,
/// in unspecified directory-walk order. Fails if a directory cannot be read.
pub(super) fn find_proto_files(dir: &str, proto_ext: &OsStr) -> Result<Vec<String>, String> {
    let mut files = Vec::new();
    let dir_iterator =
        read_dir(dir).map_err(|e| format!("Error reading directory \"{dir}\": {e}"))?;
    for entry in dir_iterator {
        let entry_path = entry
            .map_err(|e| format!("Error reading directory \"{dir}\": {e}"))?
            .path();
        if entry_path.is_dir() {
            files.append(&mut find_proto_files(
                &entry_path.to_string_lossy(),
                proto_ext,
            )?);
        } else if entry_path.extension().is_some_and(|ext| ext == proto_ext) {
            files.push(entry_path.to_string_lossy().into_owned());
        }
    }
    Ok(files)
}

/// Returns the `package` name of each file, sorted so that `proto.rs` is
/// deterministic. Fails if a file cannot be read or has no `package` line.
pub(super) fn collect_packages(proto_files: &[String]) -> Result<Vec<String>, String> {
    let mut packages = Vec::with_capacity(proto_files.len());
    for path in proto_files {
        let contents = fs::read_to_string(path)
            .map_err(|e| format!("Failed to read proto file \"{path}\": {e}"))?;
        let pkg = extract_package(&contents)
            .ok_or_else(|| format!("No `package` declaration found in \"{path}\""))?;
        packages.push(pkg);
    }
    packages.sort();
    Ok(packages)
}

// Returns the name from the first `package <name>;` line, or `None` if there is
// no such line or the name is empty. This is a line-based scan, not a proto
// parser: it assumes the declaration sits on its own line, which holds for the
// bundled protos.
fn extract_package(proto_contents: &str) -> Option<String> {
    proto_contents.lines().find_map(|line| {
        let trimmed = line.trim();
        let rest = trimmed.strip_prefix("package ")?;
        let pkg = rest.trim_end_matches(';').trim().to_string();
        if pkg.is_empty() { None } else { Some(pkg) }
    })
}
