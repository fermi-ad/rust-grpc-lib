//! Tests for [`super::Caller`] and its redaction of the token.

use super::*;

use crate::testing::claims_with_roles;

const TOKEN: &str = "eyJhbGciOi.secret-payload.signature";

#[test]
fn caller_debug_shows_the_claims_and_never_the_token() {
    let caller = Caller::new(claims_with_roles("alice", &["operator"]), TOKEN);

    let shown = format!("{caller:?}");

    assert!(!shown.contains(TOKEN), "{shown}");
    assert!(!shown.contains("secret-payload"), "{shown}");
    assert!(shown.contains("redacted"), "{shown}");
    assert!(shown.contains("alice"), "{shown}");
}
