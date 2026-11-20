// The proof that a call was authenticated is private to the library: nothing
// outside it can build a `RequestContext`, so a handler cannot be handed
// forged claims.
use rust_grpc_lib::auth::{KeycloakClaims, RequestContext};
use rust_grpc_lib::testing::claims_with_roles;

fn forge(claims: KeycloakClaims) -> RequestContext<()> {
    RequestContext {
        claims,
        message: (),
    }
}

fn main() {
    let _ = forge(claims_with_roles("mallory", &["admin"]));
}
