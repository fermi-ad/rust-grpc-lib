//! `trybuild` compile-fail cases for `#[access_controlled_service]`, the
//! server builder and the auth internals: one program per
//! `compile_fail_cases/*.rs`, with its expected diagnostic in a sibling
//! `.stderr`.
//!
//! - New case, or diagnostics changed (including after a rustc bump): run
//!   `TRYBUILD=overwrite cargo test -p integration_tests compile_fail` and
//!   review the `.stderr` diff before committing.
//! - `.stderr` files record line and column numbers, so adding or removing
//!   any line in a case, even a comment, breaks its snapshot.
//! - One case only: `cargo test -p integration_tests compile_fail --
//!   trybuild=<file>.rs`.

use trybuild::TestCases;

#[test]
fn compile_fail() {
    let cases = TestCases::new();
    cases.compile_fail("src/compile_fail_cases/*.rs");
}
