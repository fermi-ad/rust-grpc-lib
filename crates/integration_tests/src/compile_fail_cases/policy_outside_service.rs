// A policy attribute only means something inside an `impl` block annotated
// with `#[keycloak_authenticated_service]`. Anywhere else it would silently
// skip the check, so it is an error.
use rust_grpc_lib::{any_authenticated, roles};
use tonic::{Request, Response, Status};

struct Server;

impl Server {
    #[roles(any("admin"))]
    async fn guarded(&self, request: Request<String>) -> Result<Response<String>, Status> {
        Ok(Response::new(request.into_inner()))
    }

    #[any_authenticated]
    async fn authenticated(&self, request: Request<String>) -> Result<Response<String>, Status> {
        Ok(Response::new(request.into_inner()))
    }
}

fn main() {}
