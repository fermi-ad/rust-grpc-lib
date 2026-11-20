//! JWT authentication and authorization, including the `rust-auth-lib`
//! re-exports, so you need no direct dependency on that crate.
//!
//! - **Client** (always compiled): [`ClientJwtInterceptor`] attaches a
//!   [`TokenProvider`]'s token to every call; [`AuthenticatedClient`] is the
//!   transport of a client built with `authenticated`.
//! - **Server** (`server` feature): the handler parameters `RequestContext`
//!   and `PublicContext` (both dereference to `CallInfo`), and the policy
//!   types `ServicePolicy`, `PolicyTable`, `Access` and `RoleMode`.
//!
//! # How access is enforced
//!
//! `#[access_controlled_service]` emits a `ServicePolicy` table from each
//! method's `#[roles(..)]`, `#[any_authenticated]` or `#[public]`. It checks
//! nothing itself. `GrpcServer::add_service` wraps the server in a gate that,
//! before any handler runs, validates the token, checks realm roles and strips
//! the `authorization` header. It fails closed: an unannotated type cannot be
//! served, and a method missing from the table is denied. See
//! `docs/server.md` for the status codes.
//!
//! # Observability
//!
//! The gate records `access` and, once a token validates, `user` on the `rpc`
//! span. A role denial logs a `warn` event on target `rust_grpc_lib::authz`
//! with `user`, `rpc.service`, `rpc.method`, `required_roles`, `role_mode`
//! and `remote_addr`. Tokens, other claims and held roles are never logged.
//!
//! # Re-exports from `rust-auth-lib`
//!
//! [`TokenProvider`], [`TokenValidator`], [`Claims`], [`Ephemeral`],
//! [`FileTokenProvider`], [`ForwardedToken`], [`KeyValidator`],
//! [`KeyValidatorConfig`], [`AuthError`], [`ConfigError`], [`TokenError`] and
//! [`KeycloakClaims`]. `KeycloakClientCredentialsProvider` is not
//! re-exported; depend on `rust-auth-lib` directly to use it.
//!
//! [`ForwardedToken`] carries a **user's** credentials: a service gets one from
//! `ctx.token()` (the caller's token) to call another service as that user,
//! and a tool or test that already holds a user's token can build one with
//! `ForwardedToken::new`. A service's own identity is a [`FileTokenProvider`].

// Server items are code spans, not links: they are `cfg`-gated, so links
// would break in a docs build without `server`.

#[cfg(any(feature = "server", test))]
pub(crate) mod caller;
#[cfg(any(feature = "server", test))]
pub(crate) mod context;
#[cfg(any(feature = "server", test))]
pub(crate) mod gate;
pub(crate) mod interceptor;
#[cfg(any(feature = "server", test))]
pub(crate) mod policy;

pub use rust_auth_lib::keycloak::KeycloakClaims;
pub use rust_auth_lib::{
    AuthError, Claims, ConfigError, Ephemeral, FileTokenProvider, ForwardedToken, KeyValidator,
    KeyValidatorConfig, TokenError, TokenProvider, TokenValidator,
};

pub use interceptor::{AuthenticatedClient, ClientJwtInterceptor};

#[cfg(any(feature = "server", test))]
pub(crate) use context::RpcSpan;
#[cfg(any(feature = "server", test))]
pub use context::{CallInfo, PublicContext, RequestContext, RpcMethod};
#[cfg(any(feature = "server", test))]
pub use policy::{Access, MethodPolicy, PolicyTable, RoleMode, ServicePolicy};
