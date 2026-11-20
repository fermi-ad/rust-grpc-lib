// The proof that a call was authenticated is private to the library: nothing
// outside it can build an `AuthenticatedContext`, so a handler cannot be handed
// forged claims.
use rust_grpc_lib::auth::{KeycloakClaims, AuthenticatedContext};
use rust_grpc_lib::testing::claims_with_roles;

fn forge(claims: KeycloakClaims) -> AuthenticatedContext<()> {
    AuthenticatedContext {
        claims,
        message: (),
    }
}

fn main() {
    let _ = forge(claims_with_roles("mallory", &["admin"]));
}
