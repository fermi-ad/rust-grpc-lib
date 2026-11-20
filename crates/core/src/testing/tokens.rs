//! Tokens signed with the public [`test_fixtures`] key pair, and a real
//! validator that trusts it, so live-server tests exercise the actual
//! signature and expiry checks.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use rust_auth_lib::test_fixtures;
use serde_json::json;

use crate::auth::{KeyValidator, KeyValidatorConfig};

#[cfg(test)]
mod tests;

/// A [`KeyValidator`] that trusts the [`test_fixtures`] key pair under key id
/// `kid`, for [`GrpcServer::builder`](crate::server::GrpcServer::builder).
///
/// **Test-only:** the private key is public, so never use this in production.
/// Accepts tokens from [`test_token`] or [`test_fixtures::make_token`] with the
/// same `kid`; rejects another `kid`, a bad signature or an expired token.
///
/// # Panics
///
/// If the fixture key set fails to load (a fixture defect).
#[must_use = "the validator is returned, not installed anywhere; pass it to `GrpcServer::builder`"]
pub fn test_validator(kid: &str) -> Arc<KeyValidator> {
    let jwks = test_fixtures::make_jwks_json_str(kid);
    Arc::new(
        KeyValidator::new(KeyValidatorConfig::from_jwks_str(&jwks))
            .expect("the fixture key set must load"),
    )
}

/// An RS256 token for `sub` with the given realm roles, valid for one hour and
/// accepted by [`test_validator`] with the same `kid`.
///
/// ```rust
/// use rust_grpc_lib::auth::{Claims, KeycloakClaims, TokenValidator};
/// use rust_grpc_lib::testing::{test_token, test_validator};
///
/// let token = test_token("kid", "alice", &["operator"]);
/// let claims = test_validator("kid").validate::<KeycloakClaims>(&token).unwrap();
/// assert!(claims.has_role("operator"));
/// ```
///
/// # Panics
///
/// If the fixture key fails to parse or the token fails to encode (a fixture
/// defect).
#[must_use = "the token is returned, not sent anywhere"]
pub fn test_token(kid: &str, sub: &str, roles: &[&str]) -> String {
    mint(kid, sub, roles, 3600, true)
}

/// Like [`test_token`], but expired two minutes ago (beyond the validator's
/// clock-skew leeway), for testing rejection.
///
/// # Panics
///
/// As [`test_token`].
#[must_use = "the token is returned, not sent anywhere"]
pub fn expired_test_token(kid: &str, sub: &str, roles: &[&str]) -> String {
    mint(kid, sub, roles, 120, false)
}

// `future`: valid from now for `offset_secs`. Otherwise issued
// `2 * offset_secs` ago and expired `offset_secs` ago.
fn mint(kid: &str, sub: &str, roles: &[&str], offset_secs: u64, future: bool) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the Unix epoch")
        .as_secs();
    let (iat, exp) = if future {
        (now, now + offset_secs)
    } else {
        (now - 2 * offset_secs, now - offset_secs)
    };
    let payload = json!({
        "sub": sub,
        "iat": iat,
        "exp": exp,
        "realm_access": { "roles": roles },
    });
    let key = EncodingKey::from_rsa_pem(test_fixtures::RSA_PRIVATE_PEM.as_bytes())
        .expect("the fixture private key must parse");
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(kid.to_owned());
    encode(&header, &payload, &key).expect("the token must encode")
}
