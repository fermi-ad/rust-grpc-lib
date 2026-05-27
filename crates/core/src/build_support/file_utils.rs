//! File-system helpers: proto file discovery and package-name extraction.

use std::{
    ffi::OsStr,
    fs::{self, read_dir},
};

/// Recursively find all `.proto` files under `dir` and return their paths as
/// `String`s.
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

/// Read the `package` declaration from each `.proto` file and return a sorted
/// list of package names.
///
/// Returns an error if any file cannot be read or contains no `package`
/// declaration.
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

/// Extract the proto `package` declaration from the text of a single `.proto`
/// file.
///
/// Returns `None` if no `package` line is found or the package name is empty.
fn extract_package(proto_contents: &str) -> Option<String> {
    proto_contents.lines().find_map(|line| {
        let trimmed = line.trim();
        let rest = trimmed.strip_prefix("package ")?;
        let pkg = rest.trim_end_matches(';').trim().to_string();
        if pkg.is_empty() { None } else { Some(pkg) }
    })
}

#[cfg(test)]
mod tests {
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
}
