//! Tests for the claim and context helpers in [`super`].

use super::*;

use crate::auth::Claims;

#[test]
fn claims_with_roles_sets_the_subject_and_every_role() {
    let claims = claims_with_roles("alice", &["operator", "admin"]);

    assert_eq!(claims.sub(), "alice");
    assert!(claims.has_role("operator"));
    assert!(claims.has_role("admin"));
    assert!(!claims.has_role("viewer"));
}
