#!/usr/bin/env bash
# scripts/gen-test-fixtures.sh
#
# Generates the Rust proto fixtures used by the integration tests in
# crates/integration_tests/src/integration_round_trip.rs.
#
# The generated files are committed to the repository so that `cargo test`
# does not require a build.rs or a live protoc invocation.
#
# Re-run this script whenever the interface-definitions git submodule is
# updated (i.e. after `git submodule update --remote`) to keep the fixtures
# in sync with the latest proto definitions.
#
# Usage:
#   bash scripts/gen-test-fixtures.sh
#
# Requirements:
#   - Rust toolchain with cargo
#   - The interface-definitions submodule must be checked out
#     (run `git submodule update --init` if needed)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
INTEGRATION_TESTS_DIR="${WORKSPACE_ROOT}/crates/integration_tests"
FIXTURES_DIR="${INTEGRATION_TESTS_DIR}/src/fixtures"
INTERFACE_DIR="${WORKSPACE_ROOT}/crates/core/interface-definitions"

echo "Workspace root : ${WORKSPACE_ROOT}"
echo "Fixtures output: ${FIXTURES_DIR}"

# Verify the submodule is present.
if [[ ! -d "${INTERFACE_DIR}/proto" ]]; then
    echo "ERROR: interface-definitions submodule is not checked out." >&2
    echo "       Run: git submodule update --init" >&2
    exit 1
fi

mkdir -p "${FIXTURES_DIR}"

# Write a temporary Cargo project that runs tonic-prost-build and writes
# the generated .rs files directly into FIXTURES_DIR.
TMPDIR="$(mktemp -d)"
trap 'rm -rf "${TMPDIR}"' EXIT

cat > "${TMPDIR}/Cargo.toml" << TOML
[package]
name = "gen-fixtures"
version = "0.1.0"
edition = "2024"

[[bin]]
name = "gen-fixtures"
path = "src/main.rs"

[dependencies]
protoc-bin-vendored = "3"
tonic-prost-build = "0.14"
TOML

mkdir -p "${TMPDIR}/src"
cat > "${TMPDIR}/src/main.rs" << RUST
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let interface_dir = PathBuf::from(std::env::var("INTERFACE_DIR")?);
    let proto_dir = interface_dir.join("proto");
    let out_dir = PathBuf::from(std::env::var("OUT_FIXTURES_DIR")?);

    std::fs::create_dir_all(&out_dir)?;

    // Point PROTOC at the vendored binary so no system protoc is needed.
    let protoc = protoc_bin_vendored::protoc_bin_path()
        .expect("vendored protoc must be available");
    unsafe { std::env::set_var("PROTOC", protoc); }

    // Compile the grpc-db-template example proto — it has simple unary RPCs,
    // no deprecated fields, and only imports google/protobuf/timestamp.proto
    // (a well-known type bundled with tonic-prost-build).
    tonic_prost_build::configure()
        .compile_well_known_types(true)
        .client_attribute(".", "#[derive(::rust_grpc_lib::GrpcClient)]")
        .client_attribute(".", "#[derive(::rust_grpc_lib::GrpcNoAuthClient)]")
        .out_dir(&out_dir)
        .compile_protos(
            &[proto_dir.join("controls/service/grpc-db-template/v1/example.proto")],
            &[interface_dir.clone()],
        )?;

    println!("Generated fixtures in {}", out_dir.display());
    Ok(())
}
RUST

# Build and run the generator.
INTERFACE_DIR="${INTERFACE_DIR}" \
OUT_FIXTURES_DIR="${FIXTURES_DIR}" \
cargo run --manifest-path "${TMPDIR}/Cargo.toml" --quiet

echo ""
echo "Done. Generated files:"
ls -1 "${FIXTURES_DIR}/"
echo ""
echo "Commit these files to keep the test fixtures up to date."
