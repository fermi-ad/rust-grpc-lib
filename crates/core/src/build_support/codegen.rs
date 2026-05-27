//! Drives `tonic-prost-build` to compile `.proto` files into Rust source.
//!
//! The only public-to-the-module entry point is [`compile`].  All
//! environment-variable reads for user-supplied attribute overrides live here
//! so they are co-located with the builder calls that consume them.

use std::{env, error::Error};

/// Invoke `tonic-prost-build` on the given proto files.
///
/// Reads three optional environment variables to let callers attach extra Rust
/// attributes to generated types:
///
/// | Variable | Effect |
/// |---|---|
/// | `RUST_GRPC_LIB_ENUM_ATTRIBUTES` | Passed to `enum_attribute` |
/// | `RUST_GRPC_LIB_FIELD_ATTRIBUTES` | Passed to `field_attribute` |
/// | `RUST_GRPC_LIB_TYPE_ATTRIBUTES` | Passed to `type_attribute` |
///
/// Each variable holds a semicolon-separated list of `proto.path=attribute`
/// pairs (see [`parse_attribute_pairs`]).
///
/// # Safety
///
/// This function calls [`std::env::set_var`] to point `PROTOC` at the vendored
/// binary. That is only safe when no other threads are reading the environment
/// concurrently. Cargo runs `build.rs` single-threaded, so this is safe in
/// normal usage.
pub(super) fn compile(parent_dir: &str, proto_files: &[String]) -> Result<(), Box<dyn Error>> {
    let mut builder = tonic_prost_build::configure()
        .emit_rerun_if_changed(true)
        .compile_well_known_types(true)
        .client_attribute(".", "#[derive(::rust_grpc_lib::GrpcClient)]");

    for (path, attribute) in
        parse_attribute_pairs(&env::var("RUST_GRPC_LIB_ENUM_ATTRIBUTES").unwrap_or_default())
            .map_err(|e| format!("RUST_GRPC_LIB_ENUM_ATTRIBUTES: {e}"))?
    {
        builder = builder.enum_attribute(path, attribute);
    }

    for (path, attribute) in
        parse_attribute_pairs(&env::var("RUST_GRPC_LIB_FIELD_ATTRIBUTES").unwrap_or_default())
            .map_err(|e| format!("RUST_GRPC_LIB_FIELD_ATTRIBUTES: {e}"))?
    {
        builder = builder.field_attribute(path, attribute);
    }

    for (path, attribute) in
        parse_attribute_pairs(&env::var("RUST_GRPC_LIB_TYPE_ATTRIBUTES").unwrap_or_default())
            .map_err(|e| format!("RUST_GRPC_LIB_TYPE_ATTRIBUTES: {e}"))?
    {
        builder = builder.type_attribute(path, attribute);
    }

    unsafe {
        // SAFETY: build scripts run single-threaded; `set_var` is only unsafe
        // in multi-threaded contexts.
        env::set_var("PROTOC", protoc_bin_vendored::protoc_bin_path()?);
    }

    builder.compile_protos(proto_files, &[parent_dir.to_string()])?;

    Ok(())
}

/// Parse a semicolon-separated list of `proto.path=attribute` pairs.
///
/// An empty string produces an empty `Vec`.  Only the first `=` in each entry
/// is treated as the separator, so attribute values may themselves contain `=`.
///
/// # Errors
///
/// Returns an error string if any entry is missing the `=` separator.
fn parse_attribute_pairs(raw: &str) -> Result<Vec<(String, String)>, String> {
    raw.split(';')
        .filter(|s| !s.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((path, attr)) => Ok((path.trim().to_string(), attr.trim().to_string())),
            None => Err(format!(
                "Malformed attribute entry: expected `proto.path=attribute`, got `{pair}`"
            )),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // parse_attribute_pairs
    // -----------------------------------------------------------------------

    #[test]
    fn parse_attributes_empty_string() {
        assert_eq!(parse_attribute_pairs("").unwrap(), vec![]);
    }

    #[test]
    fn parse_attributes_single_pair() {
        let result = parse_attribute_pairs("foo.bar=#[derive(Debug)]").unwrap();
        assert_eq!(
            result,
            vec![("foo.bar".to_string(), "#[derive(Debug)]".to_string())]
        );
    }

    #[test]
    fn parse_attributes_multiple_pairs() {
        let raw = "a.b=#[derive(Clone)];c.d=#[serde(rename_all=\"snake_case\")]";
        let result = parse_attribute_pairs(raw).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(
            result[0],
            ("a.b".to_string(), "#[derive(Clone)]".to_string())
        );
        assert_eq!(
            result[1],
            (
                "c.d".to_string(),
                "#[serde(rename_all=\"snake_case\")]".to_string()
            )
        );
    }

    #[test]
    fn parse_attributes_trims_whitespace() {
        let result = parse_attribute_pairs("  foo.bar  =  #[attr]  ").unwrap();
        assert_eq!(result, vec![("foo.bar".to_string(), "#[attr]".to_string())]);
    }

    #[test]
    fn parse_attributes_trailing_semicolon_ignored() {
        let result = parse_attribute_pairs("a.b=#[x];").unwrap();
        assert_eq!(result, vec![("a.b".to_string(), "#[x]".to_string())]);
    }

    #[test]
    fn parse_attributes_missing_equals_returns_error() {
        let err = parse_attribute_pairs("no-equals-here").unwrap_err();
        assert!(
            err.contains("no-equals-here"),
            "error should mention the bad entry: {err}"
        );
    }

    #[test]
    fn parse_attributes_first_equals_is_separator() {
        // Value itself contains '=' — only the first '=' is the separator.
        let result = parse_attribute_pairs("p=#[serde(rename=\"a=b\")]").unwrap();
        assert_eq!(
            result,
            vec![("p".to_string(), "#[serde(rename=\"a=b\")]".to_string())]
        );
    }
}
