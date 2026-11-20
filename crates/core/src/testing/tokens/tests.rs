//! Tests for the token minting helpers in [`super`].

use super::*;

use crate::auth::{Claims, KeycloakClaims, TokenError, TokenValidator};

fn validate(validator: &KeyValidator, token: &str) -> Result<KeycloakClaims, TokenError> {
    validator.validate::<KeycloakClaims>(token)
}

#[test]
fn a_test_token_validates_with_its_subject_and_roles() {
    let validator = test_validator("kid-a");

    let claims = validate(&validator, &test_token("kid-a", "alice", &["operator"])).unwrap();

    assert_eq!(claims.sub(), "alice");
    assert!(claims.has_role("operator"));
    assert!(!claims.has_role("admin"));
}

#[test]
fn an_expired_test_token_is_rejected() {
    let validator = test_validator("kid-b");

    let result = validate(&validator, &expired_test_token("kid-b", "alice", &[]));

    assert!(result.is_err(), "an expired token must not validate");
}

#[test]
fn a_token_for_another_key_id_is_rejected() {
    let validator = test_validator("kid-c");

    let result = validate(&validator, &test_token("some-other-kid", "alice", &[]));

    assert!(result.is_err());
}
