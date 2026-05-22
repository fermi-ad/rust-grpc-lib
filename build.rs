use std::{env, error::Error, ffi::OsStr, fs::read_dir};

const PROTO_DIR: &str = "extern";
const PROTO_EXT: &str = "proto";

fn main() -> Result<(), Box<dyn Error>> {
    unsafe {
        // SAFETY: build scripts run single-threaded; `set_var` is only unsafe in multi-threaded contexts.
        std::env::set_var("PROTOC", protoc_bin_vendored::protoc_bin_path()?);
    }

    let mut builder = tonic_prost_build::configure()
        .emit_rerun_if_changed(true)
        .compile_well_known_types(true);

    for (path, attribute) in parse_attributes("RUST_GRPC_LIB_ENUM_ATTRIBUTES")? {
        builder = builder.enum_attribute(path, attribute);
    }

    for (path, attribute) in parse_attributes("RUST_GRPC_LIB_FIELD_ATTRIBUTES")? {
        builder = builder.field_attribute(path, attribute);
    }

    for (path, attribute) in parse_attributes("RUST_GRPC_LIB_TYPE_ATTRIBUTES")? {
        builder = builder.type_attribute(path, attribute);
    }

    builder.compile_protos(
        &find_proto_files(PROTO_DIR, OsStr::new(PROTO_EXT))?,
        &[PROTO_DIR.to_string()],
    )?;

    Ok(())
}

fn find_proto_files(dir: &str, proto_ext: &OsStr) -> Result<Vec<String>, String> {
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

fn parse_attributes(var_name: &str) -> Result<Vec<(String, String)>, String> {
    env::var(var_name)
        .unwrap_or_default()
        .split(';')
        .filter(|s| !s.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((path, attr)) => Ok((path.trim().to_string(), attr.trim().to_string())),
            None => Err(format!(
                "Malformed entry in {var_name}: expected `proto.path=attribute`, got `{pair}`"
            )),
        })
        .collect()
}
