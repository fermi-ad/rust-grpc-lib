//! Procedural macros for `rust-grpc-lib`, which re-exports them.
//!
//! - Clients: [`GrpcClient`] adds two constructors to a tonic client:
//!   `public(endpoint)`, which sends no token, and
//!   `authenticated(endpoint, provider)`, which attaches the provider's token
//!   to every request.
//! - Services: [`access_controlled_service`] on an `impl Trait for Type` block
//!   requires every method to declare one access policy, [`roles`],
//!   [`any_authenticated`] or [`public`], and emits the table that
//!   `GrpcServer::add_service` enforces.
//!
//! The consuming crate must depend on `rust-grpc-lib` under that name (no
//! `package = ".."` rename): the emitted code names only `::rust_grpc_lib`,
//! which re-exports the tonic it needs, so no direct `tonic` dependency is
//! required.
//!
//! The [service template](https://github.com/fermi-ad/rust-grpc-service-template)
//! is a runnable example service; `docs/server.md` and `docs/client.md` cover
//! both halves in depth.
// The dev-dependency `rust-grpc-lib` is used only by the doctests.
#![cfg_attr(test, allow(unused_crate_dependencies))]

// Maintainer notes: a proc-macro crate must define its macros at the crate
// root, so this file holds only entry points and public docs. Expansion lives
// in `client` and `service` (and its submodules), whose module docs cover
// internals and security invariants. The doctests need `rust-grpc-lib`
// (`server`) as a dev-dependency, since the expansion names it and it
// re-exports `tonic`. That is a dev-dependency cycle Cargo allows; doctests
// link this crate's normal build, so there is one copy.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;

use crate::service::Kind as PolicyKind;

mod client;
mod service;

/// Derive that adds two constructors to a tonic client, both taking a channel
/// from the process-wide pool.
///
/// - `public(endpoint: &str) -> Result<Self, ChannelError>` connects without a
///   token, for calling methods the server marks `#[public]`. Methods behind a
///   policy answer `Unauthenticated`.
/// - `authenticated<P: TokenProvider>(endpoint: &str, provider: P) ->
///   Result<Name<AuthenticatedClient<P>>, ChannelError>` wraps the channel in
///   `AuthenticatedClient` with a `ClientJwtInterceptor`, so every request
///   carries a token from `provider`. Use it for methods behind a policy.
///
/// The target method's policy decides which constructor to use. The server's
/// gate (installed by `GrpcServer::add_service`) is the authority: a gated
/// method called without a valid token answers `Unauthenticated`.
///
/// Derive it on a type with one type parameter and a `new(inner)` constructor,
/// as tonic clients have; only the type's name is used. The derive raises no
/// errors of its own: a type without that shape fails to type-check the
/// generated `impl`.
///
/// # Name collisions
///
/// The constructors are inherent associated functions, so an RPC whose
/// snake_case method name is `public` or `authenticated` collides with them
/// (`E0592`, duplicate definitions). Rename the RPC in the `.proto` file.
#[proc_macro_derive(GrpcClient)]
pub fn grpc_client_derive(input: TokenStream) -> TokenStream {
    into_token_stream(client::expand_grpc_client(input.into()))
}

/// Access policy: the caller needs a valid token with some or all of the
/// listed realm roles.
///
/// - `#[roles(any("a", "b"))]`: at least one listed role.
/// - `#[roles(all("a", "b"))]`: every listed role.
///
/// Each role is a string literal or a path to a `&str` constant. The method
/// takes a `RequestContext<T>`. See [`access_controlled_service`] for an
/// example and the errors a malformed list raises.
///
/// Policy attributes are valid only on a method inside an
/// [`access_controlled_service`] block, which strips them (so they need no
/// import there). Anywhere else they are a compile error, so a policy is
/// never silently ignored.
#[proc_macro_attribute]
pub fn roles(_attr: TokenStream, item: TokenStream) -> TokenStream {
    unstripped(PolicyKind::Roles, item)
}

/// Access policy: any caller with a valid token; no role is needed.
///
/// The method takes a `RequestContext<T>`. Takes no arguments; placement rules
/// as for [`roles`].
#[proc_macro_attribute]
pub fn any_authenticated(_attr: TokenStream, item: TokenStream) -> TokenStream {
    unstripped(PolicyKind::AnyAuthenticated, item)
}

/// Access policy: open to everyone, with or without a token.
///
/// The only way to make a method reachable without a token, and easy to grep
/// for. The method takes a `PublicContext<T>`, which carries the message and
/// call information but no claims or token, even if the caller sent one.
/// Takes no arguments; placement rules as for [`roles`].
#[proc_macro_attribute]
pub fn public(_attr: TokenStream, item: TokenStream) -> TokenStream {
    unstripped(PolicyKind::Public, item)
}

/// Attribute for an `impl Trait for Type` block of a tonic service trait:
/// every method must declare exactly one access policy, [`roles`],
/// [`any_authenticated`] or [`public`].
///
/// The macro declares policies; it authenticates nothing and emits no check.
/// The gate that `GrpcServer::add_service` installs enforces the table before
/// any handler runs and denies any method not in it.
///
/// # Usage
///
/// Write it **above** `#[rust_grpc_lib::async_trait]`, so it sees the `async fn`
/// signatures before `async_trait` rewrites them. Each method's first
/// non-`self` parameter is `RequestContext<T>` (`roles`, `any_authenticated`)
/// or `PublicContext<T>` (`public`), matched by its last path segment and
/// bound to a plain name such as `ctx` or `mut ctx`.
///
/// ```rust
/// # use std::marker::PhantomData;
/// # use grpc_macro::access_controlled_service;
/// # use rust_grpc_lib::auth::{PublicContext, RequestContext};
/// # use rust_grpc_lib::tonic::{Request, Response, Status};
/// # struct Msg;
/// # #[rust_grpc_lib::async_trait]
/// # trait MyService: Send + Sync + 'static {
/// #     async fn set_data(&self, r: Request<Msg>) -> Result<Response<Msg>, Status>;
/// #     async fn reset(&self, r: Request<Msg>) -> Result<Response<Msg>, Status>;
/// #     async fn ping(&self, r: Request<Msg>) -> Result<Response<Msg>, Status>;
/// #     async fn version(&self, r: Request<Msg>) -> Result<Response<Msg>, Status>;
/// # }
/// # struct MyServiceServer<T>(PhantomData<T>);
/// # struct MyServer;
/// # mod roles { pub const OPERATOR: &str = "operator"; pub const ADMIN: &str = "admin"; }
/// #[access_controlled_service]
/// #[rust_grpc_lib::async_trait]
/// impl MyService for MyServer {
///     #[roles(any("operator", "admin"))]
///     async fn set_data(&self, ctx: RequestContext<Msg>) -> Result<Response<Msg>, Status> {
///         # let _ = ctx;
///         // ...
///         # Ok(Response::new(Msg))
///     }
///
///     #[roles(all(roles::OPERATOR, roles::ADMIN))]
///     async fn reset(&self, ctx: RequestContext<Msg>) -> Result<Response<Msg>, Status> {
///         # let _ = ctx;
///         // ...
///         # Ok(Response::new(Msg))
///     }
///
///     #[any_authenticated]
///     async fn ping(&self, ctx: RequestContext<Msg>) -> Result<Response<Msg>, Status> {
///         # let _ = ctx;
///         // ...
///         # Ok(Response::new(Msg))
///     }
///
///     #[public]
///     async fn version(&self, ctx: PublicContext<Msg>) -> Result<Response<Msg>, Status> {
///         # let _ = ctx;
///         // ...
///         # Ok(Response::new(Msg))
///     }
/// }
/// # fn main() {}
/// ```
///
/// # Arguments
///
/// `server = path::FooServer` names the generated server type, without generic
/// arguments. By default it is the trait path with its last segment `Foo`
/// renamed to `FooServer`; pass it when that name is not in scope.
///
/// # Generated code
///
/// - The block, with policy attributes stripped and each context parameter
///   changed to the `::rust_grpc_lib::__private::tonic::Request<T>` the trait
///   expects under a hidden
///   name. The body first builds your context from it with
///   `try_from_request`, which fails closed with `Internal` unless the call
///   came through `add_service`.
/// - `impl ServicePolicy<FooServer<Type>> for Type`, the policy table that
///   `add_service` requires. Wire names match the Rust names ignoring case,
///   `_` and `r#`.
///
/// # Compile errors
///
/// - An unknown argument, or `server` given twice.
/// - An inherent (non-trait) `impl` block.
/// - Generic arguments on `server = ..`, or on the trait path when `server`
///   is absent (the trait path is not checked when `server` is given).
/// - The attribute placed below `#[async_trait]`.
/// - A method with no policy, or with more than one.
/// - `#[roles]` not of the form `roles(any(..))` or `roles(all(..))`: another
///   mode, an empty role list, trailing tokens, or a role that is neither a
///   string literal nor a path.
/// - Arguments on `#[any_authenticated]` or `#[public]`.
/// - A method with no request parameter.
/// - A parameter that is not a context, such as a plain `tonic::Request<T>`.
/// - A context without a message type, such as a bare `RequestContext`.
/// - The wrong context for the policy, such as `RequestContext` on a
///   `#[public]` method.
/// - A context bound to anything but a plain name (destructuring, `ref`, `@`).
/// - Two methods whose names are equal ignoring case, `_` and `r#`: the gate
///   could not tell them apart.
///
/// Errors from different methods are reported together.
#[proc_macro_attribute]
pub fn access_controlled_service(attr: TokenStream, item: TokenStream) -> TokenStream {
    into_token_stream(service::expand(attr.into(), item.into()))
}

// Turns an expansion result into the macro's output, rendering an error as a
// `compile_error!` at the span it carries.
fn into_token_stream(result: syn::Result<TokenStream2>) -> TokenStream {
    result.unwrap_or_else(syn::Error::into_compile_error).into()
}

// The output of a policy attribute that reached its own expansion, which
// means `#[access_controlled_service]` did not strip it: a
// `compile_error!` followed by the item unchanged, so the user sees only this
// error rather than a cascade.
fn unstripped(kind: PolicyKind, item: TokenStream) -> TokenStream {
    let mut output = TokenStream::from(kind.unstripped_error().into_compile_error());
    output.extend(item);
    output
}
