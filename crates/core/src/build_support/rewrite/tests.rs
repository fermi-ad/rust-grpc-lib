//! Tests for [`super::qualify_tonic_paths`] and
//! [`super::rewrite_generated_files`].
//!
//! Covers: each tonic-build path shape that must be rewritten, every shape that
//! must be left alone (identifier boundaries, qualified paths, comments),
//! idempotence, and the file-level behavior (line endings, missing files,
//! untouched neighbors).

use std::{env, fs, path::PathBuf, process};

use super::*;

const QUALIFIED: &str = "::rust_grpc_lib::__private::tonic::";

fn rewrite(line: &str) -> String {
    qualify_tonic_paths(line).into_owned()
}

// -----------------------------------------------------------------------
// qualify_tonic_paths: rewritten
// -----------------------------------------------------------------------

#[test]
fn rewrites_a_path_at_the_start_of_a_line() {
    assert_eq!(
        rewrite("tonic::client::Grpc<T>"),
        format!("{QUALIFIED}client::Grpc<T>")
    );
}

#[test]
fn rewrites_a_path_after_indentation() {
    assert_eq!(
        rewrite("    tonic::codegen::*;"),
        format!("    {QUALIFIED}codegen::*;")
    );
}

#[test]
fn rewrites_paths_after_each_kind_of_delimiter() {
    // The shapes tonic-build emits: after `<`, `(`, `:`+space, `=`, `,`, `+`,
    // `&`, `>` and a space.
    let cases = [
        ("impl X<tonic::transport::Channel> {", "impl X<"),
        ("fn f(x: tonic::Request<T>)", "fn f(x: "),
        ("let g = tonic::server::Grpc::new(c);", "let g = "),
        ("F: tonic::service::Interceptor,", "F: "),
        ("A, tonic::Status", "A, "),
        ("T: Send + tonic::Foo", "T: Send + "),
        ("&tonic::Status", "&"),
        ("-> tonic::Status", "-> "),
    ];
    for (input, prefix) in cases {
        let output = rewrite(input);
        assert!(
            output.contains(&format!("{prefix}{QUALIFIED}")),
            "{input:?} became {output:?}"
        );
    }
}

#[test]
fn rewrites_every_occurrence_on_a_line() {
    assert_eq!(
        rewrite("fn f(r: tonic::Request<A>) -> tonic::Response<B>"),
        format!("fn f(r: {QUALIFIED}Request<A>) -> {QUALIFIED}Response<B>")
    );
}

#[test]
fn rewrites_adjacent_occurrences_separated_by_one_character() {
    assert_eq!(
        rewrite("(tonic::A,tonic::B)"),
        format!("({QUALIFIED}A,{QUALIFIED}B)")
    );
}

#[test]
fn rewrites_a_path_before_a_turbofish() {
    assert_eq!(
        rewrite("let x = tonic::Foo::<u8>::new();"),
        format!("let x = {QUALIFIED}Foo::<u8>::new();")
    );
}

#[test]
fn rewrites_use_declarations() {
    assert_eq!(
        rewrite("    use tonic::codegen::http::Uri;"),
        format!("    use {QUALIFIED}codegen::http::Uri;")
    );
}

#[test]
fn keeps_the_line_ending() {
    assert_eq!(rewrite("tonic::A\n"), format!("{QUALIFIED}A\n"));
    assert_eq!(rewrite("tonic::A\r\n"), format!("{QUALIFIED}A\r\n"));
}

// -----------------------------------------------------------------------
// qualify_tonic_paths: left alone
// -----------------------------------------------------------------------

#[test]
fn leaves_tonic_prost_alone() {
    let line = "let codec = tonic_prost::ProstCodec::default();";
    assert_eq!(rewrite(line), line);
}

#[test]
fn leaves_longer_identifiers_alone() {
    for line in [
        "mytonic::x",
        "my_tonic::x",
        "tonic2::x",
        "_tonic::x",
        "let a = Xtonic::y;",
        "tonic_health::pb::X",
        "tonicity::x",
    ] {
        assert_eq!(rewrite(line), line);
    }
}

#[test]
fn leaves_identifiers_with_non_ascii_neighbors_alone() {
    // `é` is an identifier character, so `étonic::x` is a different path.
    let line = "étonic::x";
    assert_eq!(rewrite(line), line);
}

#[test]
fn leaves_already_qualified_paths_alone() {
    for line in [
        "::tonic::Status",
        "let x: ::tonic::Request<T>;",
        "a::tonic::x",
        "crate::tonic::x",
        "::rust_grpc_lib::__private::tonic::Status",
        "    ::rust_grpc_lib::__private::tonic::codegen::*;",
    ] {
        assert_eq!(rewrite(line), line);
    }
}

#[test]
fn leaves_method_calls_and_raw_identifiers_alone() {
    for line in ["x.tonic::<T>()", "x .tonic::<T>()", "r#tonic::x"] {
        assert_eq!(rewrite(line), line);
    }
}

#[test]
fn leaves_tonic_without_a_path_separator_alone() {
    for line in ["tonic", "let tonic = 1;", "tonic:", "tonic :: X", "tonic:x"] {
        assert_eq!(rewrite(line), line);
    }
}

#[test]
fn leaves_comment_lines_alone() {
    for line in [
        "// tonic::Status is returned",
        "    /// Calls tonic::client::Grpc.",
        "//! Uses tonic::transport.",
        "\t// tonic::Foo",
    ] {
        assert_eq!(rewrite(line), line);
    }
}

#[test]
fn returns_a_borrowed_line_when_nothing_changes() {
    assert!(matches!(
        qualify_tonic_paths("let a = 1;"),
        Cow::Borrowed(_)
    ));
    assert!(matches!(
        qualify_tonic_paths("// tonic::X"),
        Cow::Borrowed(_)
    ));
    assert!(matches!(qualify_tonic_paths("tonic::X"), Cow::Owned(_)));
}

// -----------------------------------------------------------------------
// qualify_tonic_paths: mixed lines and idempotence
// -----------------------------------------------------------------------

#[test]
fn rewrites_only_the_paths_on_a_mixed_line() {
    assert_eq!(
        rewrite("let c = tonic_prost::ProstCodec::default(); let g = tonic::server::Grpc::new(c);"),
        format!(
            "let c = tonic_prost::ProstCodec::default(); let g = {QUALIFIED}server::Grpc::new(c);"
        )
    );
}

#[test]
fn is_idempotent() {
    let lines = [
        "tonic::A",
        "    use tonic::codegen::*;",
        "fn f(r: tonic::Request<A>) -> tonic::Response<B>",
        "tonic_prost::ProstCodec",
        "::tonic::A",
        "// tonic::A",
    ];
    for line in lines {
        let once = rewrite(line);
        assert_eq!(rewrite(&once), once, "not idempotent for {line:?}");
    }
}

// -----------------------------------------------------------------------
// rewrite_generated_files
// -----------------------------------------------------------------------

// A scratch directory unique to one test.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = env::temp_dir().join(format!("rust-grpc-lib-rewrite-{name}-{}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create scratch dir");
        Scratch(dir)
    }

    fn write(&self, name: &str, contents: &str) {
        fs::write(self.0.join(name), contents).expect("write scratch file");
    }

    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.0.join(name)).expect("read scratch file")
    }

    fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(&self.0)
            .expect("read scratch dir")
            .map(|entry| entry.expect("dir entry").file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn rewrites_the_files_of_the_given_packages() {
    let dir = Scratch::new("packages");
    dir.write(
        "common.alarm.rs",
        "use tonic::codegen::*;\n// tonic::Doc\nlet c = tonic_prost::ProstCodec::default();\n",
    );
    dir.write("services.example.rs", "x: tonic::Status\r\ny: tonic::Code");

    rewrite_generated_files(
        &dir.0,
        &["common.alarm".to_string(), "services.example".to_string()],
    )
    .expect("rewrite succeeds");

    assert_eq!(
        dir.read("common.alarm.rs"),
        format!(
            "use {QUALIFIED}codegen::*;\n// tonic::Doc\nlet c = tonic_prost::ProstCodec::default();\n"
        )
    );
    // CRLF endings and a missing final newline survive.
    assert_eq!(
        dir.read("services.example.rs"),
        format!("x: {QUALIFIED}Status\r\ny: {QUALIFIED}Code")
    );
}

#[test]
fn rewrites_the_well_known_types_file_when_present() {
    let dir = Scratch::new("well-known");
    dir.write("google.protobuf.rs", "x: tonic::Status\n");

    rewrite_generated_files(&dir.0, &[]).expect("rewrite succeeds");

    assert_eq!(
        dir.read("google.protobuf.rs"),
        format!("x: {QUALIFIED}Status\n")
    );
}

#[test]
fn leaves_unlisted_files_alone() {
    let dir = Scratch::new("unlisted");
    dir.write("common.alarm.rs", "tonic::A\n");
    dir.write("proto.rs", "tonic::B\n");
    dir.write("other.rs", "tonic::C\n");

    rewrite_generated_files(&dir.0, &["common.alarm".to_string()]).expect("rewrite succeeds");

    assert_eq!(dir.read("common.alarm.rs"), format!("{QUALIFIED}A\n"));
    assert_eq!(dir.read("proto.rs"), "tonic::B\n");
    assert_eq!(dir.read("other.rs"), "tonic::C\n");
}

#[test]
fn leaves_no_temporary_files_behind() {
    let dir = Scratch::new("temp");
    dir.write("a.rs", "tonic::A\n");

    rewrite_generated_files(&dir.0, &["a".to_string()]).expect("rewrite succeeds");

    assert_eq!(dir.names(), ["a.rs"]);
}

#[test]
fn finds_files_by_escaped_package_names() {
    // prost names the file after the escaped segments (`r#type`).
    let dir = Scratch::new("escaped");
    dir.write("third_party.r#type.rs", "tonic::A\n");

    rewrite_generated_files(&dir.0, &["third_party.type".to_string()]).expect("rewrite succeeds");

    assert_eq!(dir.read("third_party.r#type.rs"), format!("{QUALIFIED}A\n"));
}

#[test]
fn a_missing_package_file_is_an_error_naming_the_file() {
    let dir = Scratch::new("missing");

    let error =
        rewrite_generated_files(&dir.0, &["common.alarm".to_string()]).expect_err("must fail");

    assert!(error.contains("common.alarm.rs"), "{error}");
}
