//! [`Caller`]: the crate-private proof that a call was authenticated.

// Security invariant: a `Caller` is the only proof of authentication that
// `RequestContext::try_from_request` accepts. Only the gate (and
// `testing::authenticated_request`) inserts one, and `pub(crate)` stops other
// crates from forging it. Do not widen its or its fields' visibility.
//
// The token is kept so a handler can forward it, inside `Redacted` so that
// `Debug` stays derivable without leaking it.

use std::{fmt, ops::Deref};

use rust_auth_lib::{ForwardedToken, keycloak::KeycloakClaims};

#[cfg(test)]
mod tests;

/// The validated claims of a call and the token they came from.
#[derive(Clone, Debug)]
pub(crate) struct Caller {
    pub(crate) claims: KeycloakClaims,
    pub(crate) token: Redacted<ForwardedToken>,
}

impl Caller {
    /// Pair validated `claims` with the `token` they were read from.
    pub(crate) fn new(claims: KeycloakClaims, token: impl Into<String>) -> Self {
        Self {
            claims,
            token: Redacted(ForwardedToken::new(token)),
        }
    }
}

/// Holds a secret; `Debug` prints `<redacted>`.
#[derive(Clone)]
pub(crate) struct Redacted<T>(T);

impl<T> Redacted<T> {
    /// Take the secret out, for a consumer that hands it on by value.
    pub(crate) fn into_inner(self) -> T {
        self.0
    }
}

impl<T> Deref for Redacted<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> From<T> for Redacted<T> {
    fn from(value: T) -> Self {
        Self(value)
    }
}

impl<T> fmt::Debug for Redacted<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}
