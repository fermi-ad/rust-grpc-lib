//! Compile-fail tests for `#[keycloak_authenticated_service]` error messages
//! and spans. Each case lives in `compile_fail_cases/` with its expected
//! compiler output in a sibling `.stderr` file.
//!
//! The `.stderr` files are rustc-version dependent. After a toolchain bump,
//! regenerate them with `TRYBUILD=overwrite cargo test -p integration_tests
//! compile_fail` and review the diff.

#[test]
fn compile_fail() {
    let cases = trybuild::TestCases::new();
    cases.compile_fail("src/compile_fail_cases/*.rs");
}
