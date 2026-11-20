//! Tests for what `Config::new` and `compile` put in the generated code.
//!
//! These run the vendored `protoc` over a tiny proto and read the generated
//! Rust, so they check what a consumer's `proto.rs` really contains: the
//! attributes on clients, and that the code names only `::rust_grpc_lib`.

use std::{env, fs, path::PathBuf};

use super::*;

const PROTO: &str = r#"syntax = "proto3";
package config_test.greeter;

message Ping {}

service Greeter {
  rpc Say(Ping) returns (Ping);
}
"#;

// Generates code for `PROTO` with `config` through the same `compile` that
// `generate_protos` uses, and returns the generated source. `name` keeps the
// scratch directories of parallel tests apart.
fn generate(name: &str, config: Config) -> String {
    let dir: PathBuf = env::temp_dir().join(format!(
        "rust-grpc-lib-build-support-{name}-{}",
        std::process::id()
    ));
    let out = dir.join("out");
    fs::create_dir_all(&out).expect("create scratch dir");
    let proto = dir.join("greeter.proto");
    fs::write(&proto, PROTO).expect("write proto");

    compile(
        dir.to_str().expect("utf-8 temp dir"),
        &[proto.to_string_lossy().into_owned()],
        &["config_test.greeter".to_string()],
        config,
        &out,
    )
    .expect("protoc, tonic-prost-build and the rewrite succeed");

    let source =
        fs::read_to_string(out.join("config_test.greeter.rs")).expect("generated file exists");
    let _ = fs::remove_dir_all(&dir);
    source
}

// The derives sit directly above the generated client struct.
fn client_derives(source: &str) -> Vec<&str> {
    source
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("#[derive(::rust_grpc_lib::"))
        .collect()
}

// The lines of `source` with a path-start `tonic::` that is not qualified.
fn unqualified_tonic_lines(source: &str) -> Vec<&str> {
    source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .filter(|line| {
            let mut rest = *line;
            let mut offset = 0;
            while let Some(at) = rest.find("tonic::") {
                let before = line[..offset + at].chars().next_back();
                let continues = before
                    .is_some_and(|c| c.is_alphanumeric() || matches!(c, '_' | ':' | '.' | '#'));
                if !continues {
                    return true;
                }
                offset += at + "tonic::".len();
                rest = &line[offset..];
            }
            false
        })
        .collect()
}

#[test]
fn new_derives_grpc_client_on_every_client() {
    let source = generate("defaults", Config::new());

    assert_eq!(
        client_derives(&source),
        ["#[derive(::rust_grpc_lib::GrpcClient)]"],
        "generated client:\n{source}"
    );
    assert!(source.contains("pub struct GreeterClient<T>"));
}

#[test]
fn generated_code_names_tonic_through_rust_grpc_lib() {
    let source = generate("tonic-paths", Config::new());

    assert!(
        source.contains("::rust_grpc_lib::__private::tonic::client::Grpc<T>"),
        "generated code:\n{source}"
    );
    assert!(
        unqualified_tonic_lines(&source).is_empty(),
        "unqualified tonic paths: {:#?}\n{source}",
        unqualified_tonic_lines(&source)
    );
}

#[test]
fn generated_code_names_the_codec_through_rust_grpc_lib() {
    let source = generate("codec", Config::new());

    assert!(
        source.contains("::rust_grpc_lib::__private::tonic_prost::ProstCodec::default()"),
        "generated code:\n{source}"
    );
    assert!(
        !source
            .lines()
            .any(|line| line.contains("tonic_prost::") && !line.contains("::__private::")),
        "unqualified tonic_prost path:\n{source}"
    );
}

#[test]
fn generated_messages_name_prost_through_rust_grpc_lib() {
    let source = generate("prost", Config::new());

    assert!(
        source.contains("::rust_grpc_lib::__private::prost::Message"),
        "generated code:\n{source}"
    );
    assert!(
        source.contains("#[prost(") && source.contains("prost_path = "),
        "the `Message` derive needs the `prost_path` attribute:\n{source}"
    );
    assert!(
        !source.contains("::prost::Message") || source.contains("__private::prost::Message"),
        "generated code:\n{source}"
    );
}
