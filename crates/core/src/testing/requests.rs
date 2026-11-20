//! Requests shaped like what the access gate hands a handler.
//!
//! `#[access_controlled_service]` rewrites a handler's context parameter to
//! `tonic::Request<T>`; these build such requests so a test can call the
//! handler directly. The gate is bypassed: this tests handler logic, not
//! access policy.

use tonic::Request;

use crate::auth::caller::Caller;
use crate::auth::{KeycloakClaims, RpcMethod};

/// Wraps `message` for calling a `#[roles]` or `#[any_authenticated]` handler
/// directly.
///
/// Attaches what the gate and trace layer would: the caller (`claims`, token
/// `test-token`) and the method `test.Service/Test`. Without the caller (for
/// example a plain `Request::new`, or [`public_request`]) the handler fails
/// closed with `Internal "authentication is not configured"`.
///
/// ```rust
/// # use std::marker::PhantomData;
/// # use rust_grpc_lib::access_controlled_service;
/// # use rust_grpc_lib::auth::RequestContext;
/// # use rust_grpc_lib::tonic::{Code, Request, Response, Status};
/// # trait Greeter {
/// #     async fn greet(&self, request: Request<String>) -> Result<Response<String>, Status>;
/// # }
/// # struct GreeterServer<T>(PhantomData<T>);
/// # struct MyGreeter;
/// use rust_grpc_lib::testing::{authenticated_request, claims_with_roles};
///
/// #[access_controlled_service]
/// impl Greeter for MyGreeter {
///     #[any_authenticated]
///     async fn greet(&self, ctx: RequestContext<String>) -> Result<Response<String>, Status> {
///         Ok(Response::new(format!("hello {}", ctx.sub())))
///     }
/// }
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() {
/// let request = authenticated_request(String::new(), claims_with_roles("alice", &[]));
/// assert_eq!(MyGreeter.greet(request).await.unwrap().into_inner(), "hello alice");
///
/// let status = MyGreeter.greet(Request::new(String::new())).await.unwrap_err();
/// assert_eq!(status.code(), Code::Internal);
/// # }
/// ```
#[must_use = "the request is returned, not sent anywhere; pass it to the handler"]
pub fn authenticated_request<T>(message: T, claims: KeycloakClaims) -> Request<T> {
    let mut request = Request::new(message);
    request
        .extensions_mut()
        .insert(Caller::new(claims, "test-token"));
    request.extensions_mut().insert(RpcMethod::for_test());
    request
}

/// Wraps `message` for calling a `#[public]` handler directly: method
/// `test.Service/Test`, no caller.
#[must_use = "the request is returned, not sent anywhere; pass it to the handler"]
pub fn public_request<T>(message: T) -> Request<T> {
    let mut request = Request::new(message);
    request.extensions_mut().insert(RpcMethod::for_test());
    request
}
