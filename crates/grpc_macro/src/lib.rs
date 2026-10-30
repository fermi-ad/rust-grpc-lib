//! Procedural macros for `rust-grpc-lib`.
//!
//! This crate exposes four public macros:
//!
//! - [`GrpcClient`] — derive macro that generates
//!   `from_endpoint_with_provider` on a tonic client struct, wiring it into
//!   the process-wide channel pool with JWT auth via `ClientJwtInterceptor`.
//! - [`GrpcNoAuthClient`] — derive macro that generates `from_endpoint` (no
//!   auth) on a tonic client struct. For test harnesses only; requires the
//!   `unauthenticated` feature on `rust-grpc-lib`.
//! - [`keycloak_authenticated_service`] — attribute macro applied to an `impl Trait for Type`
//!   block that injects Keycloak role-checking guards into methods annotated
//!   with `#[roles(...)]` and lets handlers take a `RequestContext<T>` instead
//!   of a `tonic::Request<T>`.
//! - [`roles`] — marker attribute consumed by [`keycloak_authenticated_service`]. A pass-through
//!   no-op when used without `#[keycloak_authenticated_service]` on the enclosing `impl` block.
//!
//! # Layout
//!
//! A proc-macro crate must define its macros at the crate root, so this file
//! holds only the documented entry points. Each one delegates to a module that
//! works on `proc_macro2` token streams and returns a `syn::Result`:
//!
//! | Module | Expands |
//! |---|---|
//! | `client` | `GrpcClient` and `GrpcNoAuthClient` |
//! | `service` | `keycloak_authenticated_service`, split into one submodule per concern (arguments, environment, `#[async_trait]` order check, request-parameter rewrite, `#[roles]` parsing, guard emission) |
//!
//! # Dependencies of the consumer
//!
//! The code emitted by [`keycloak_authenticated_service`] names `::tonic` and
//! `::tracing`, which resolve to whatever versions the consuming crate depends
//! on. The crate does not pin or re-export them. Three couplings cannot be
//! avoided:
//!
//! - **Crate names.** By policy every consumer depends on `rust-grpc-lib`,
//!   `tonic` and `tracing` under their normal names (no `package = ".."`
//!   renames). The emitted code is rooted at `::rust_grpc_lib`, `::tonic` and
//!   `::tracing`, and no crate-name resolution is attempted.
//! - **`tonic` compatibility.** A `RequestContext<T>` parameter is rewritten to
//!   `::tonic::Request<T>` and handed to `RequestContext::try_from_request`,
//!   and the guard returns `::tonic::Status`. The consumer's `tonic` must
//!   therefore be semver-compatible with the one `rust-grpc-lib` is built
//!   against, or the types will not unify.
//! - **`tracing` presence.** The guard always emits events (denials and
//!   missing claims), so the expansion calls `::tracing` and the consumer
//!   needs `tracing` as a direct dependency (any 0.1 works).

use proc_macro::TokenStream;

mod client;
mod service;

#[proc_macro_derive(GrpcClient)]
pub fn grpc_client_derive(input: TokenStream) -> TokenStream {
    into_token_stream(client::expand_grpc_client(input.into()))
}

#[proc_macro_derive(GrpcNoAuthClient)]
pub fn grpc_no_auth_client_derive(input: TokenStream) -> TokenStream {
    into_token_stream(client::expand_grpc_no_auth_client(input.into()))
}

/// Marker attribute consumed by [`keycloak_authenticated_service`].
///
/// When used standalone (without `#[keycloak_authenticated_service]` on the enclosing `impl`
/// block) this attribute is a pass-through no-op so that the code still
/// compiles. `#[keycloak_authenticated_service]` strips and processes it before emitting the
/// final `impl` block.
///
/// # Usage
///
/// ```rust,ignore
/// #[keycloak_authenticated_service]
/// impl MyService for MyServer {
///     #[roles(any("operator", "admin"))]
///     async fn set_data(&self, request: Request<SetDataRequest>)
///         -> Result<Response<SetDataResponse>, Status>
///     {
///         // ...
///     }
/// }
/// ```
#[proc_macro_attribute]
pub fn roles(_attr: TokenStream, item: TokenStream) -> TokenStream {
    // Pass-through; keycloak_authenticated_service strips and processes this attribute.
    item
}

/// Attribute macro applied to an `impl Trait for Type` block. It injects
/// Keycloak role-checking guards into methods annotated with `#[roles(...)]`
/// and rewrites `RequestContext<T>` parameters.
///
/// # Placement
///
/// The attribute must be written **above** `#[async_trait]`, because it has to
/// see the `async fn` signatures before `async_trait` rewrites them. The wrong
/// order is detected and reported with a dedicated compile error.
///
/// ```rust,ignore
/// #[keycloak_authenticated_service]
/// #[tonic::async_trait]
/// impl MyService for MyServer { /* ... */ }
/// ```
///
/// # Role check variants
///
/// - `#[roles(any("r1", "r2"))]` — at least one of the listed roles must be
///   present in the JWT claims.
/// - `#[roles(all("r1", "r2"))]` — every listed role must be present.
/// - No `#[roles(...)]` attribute — the method gets no role check (a valid JWT
///   is still required by the JWT validation layer, but no role check is
///   injected by this macro).
///
/// # Handler parameter forms
///
/// The first non-`self` parameter of each method may be either of:
///
/// - `tonic::Request<T>` — the method is emitted unchanged apart from the
///   guard, and reads claims from the request extensions.
/// - `RequestContext<T>` (any path whose last segment is `RequestContext`) —
///   the emitted signature takes a `tonic::Request<T>` under a hygienic name
///   and the body starts with
///   `let <name> = RequestContext::try_from_request(<request>)?;`. The role
///   guard then reuses `<name>.claims`, so claims are looked up only once.
///
/// Both forms can be mixed in one `impl` block.
///
/// # Arguments
///
/// `#[keycloak_authenticated_service(claims = SomeClaims)]` selects the claims
/// type that `Request<T>` methods read from the extensions. It defaults to
/// `rust_grpc_lib::auth::KeycloakClaims`. `SomeClaims` must implement
/// `Claims` and provide `has_role(&self, &str) -> bool`. Methods taking
/// `RequestContext<T, C>` use the `C` written in the signature.
///
/// # Denied calls
///
/// A denied call returns `Status::permission_denied("required role not
/// present")` and logs a `tracing::warn!` event with target
/// `rust_grpc_lib::authz` and the message `authorization denied`. Its fields
/// are `user` (the `sub` claim), `rpc.service` (the trait name), `rpc.method`
/// (the Rust method name), `required_roles`, `role_mode` (`any` or `all`) and,
/// when the transport reports it, `remote_addr`. The roles the caller actually
/// holds, the token and the other claims are never logged.
///
/// # Generated code shape
///
/// ```rust,ignore
/// async fn set_data(&self, __request: ::tonic::Request<SetDataRequest>)
///     -> Result<Response<SetDataResponse>, Status>
/// {
///     let ctx: RequestContext<SetDataRequest> =
///         RequestContext::try_from_request(__request)?;
///     {
///         let __claims = &ctx.claims;
///         if !["operator", "admin"].iter().any(|r| __claims.has_role(r)) {
///             // tracing::warn!(target: "rust_grpc_lib::authz", ...);
///             return Err(::tonic::Status::permission_denied("required role not present"));
///         }
///     }
///     // original body …
/// }
/// ```
#[proc_macro_attribute]
pub fn keycloak_authenticated_service(attr: TokenStream, item: TokenStream) -> TokenStream {
    into_token_stream(service::expand(attr.into(), item.into()))
}

/// Turns an expansion result into the macro's output, rendering an error as a
/// `compile_error!` at the span it carries.
fn into_token_stream(result: syn::Result<proc_macro2::TokenStream>) -> TokenStream {
    result.unwrap_or_else(syn::Error::into_compile_error).into()
}
