// A service the macro never processed cannot be served: `add_service` needs
// the policy table that `#[access_controlled_service]` emits.
use std::convert::Infallible;
use std::future::{Ready, ready};
use std::sync::Arc;
use std::task::{Context, Poll};

use rust_grpc_lib::auth::KeyValidator;
use rust_grpc_lib::server::GrpcServer;
use rust_grpc_lib::tonic::body::Body;
use rust_grpc_lib::tonic::codegen::Service;
use rust_grpc_lib::tonic::codegen::http::{Request, Response};
use rust_grpc_lib::tonic::server::NamedService;

/// A stand-in for a generated `FooServer<T>`: a valid tonic service.
struct Unannotated;
struct UnannotatedServer<T>(Arc<T>);

impl<T> Clone for UnannotatedServer<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T> UnannotatedServer<T> {
    fn new(inner: T) -> Self {
        Self(Arc::new(inner))
    }
}

impl<T> Service<Request<Body>> for UnannotatedServer<T> {
    type Response = Response<Body>;
    type Error = Infallible;
    type Future = Ready<Result<Response<Body>, Infallible>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _: Request<Body>) -> Self::Future {
        ready(Ok(Response::new(Body::empty())))
    }
}

impl<T> NamedService for UnannotatedServer<T> {
    const NAME: &'static str = "pkg.Unannotated";
}

fn wire(validator: Arc<KeyValidator>) {
    let _ = GrpcServer::builder(validator).add_service(UnannotatedServer::new, Unannotated);
}

fn main() {}
