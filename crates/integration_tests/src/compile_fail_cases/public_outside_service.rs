// `#[public]` only means something inside an `impl` block annotated with
// `#[access_controlled_service]`. Anywhere else it would silently open
// nothing, so it is an error.
use rust_grpc_lib::public;
use rust_grpc_lib::tonic::{Request, Response, Status};

struct Server;

impl Server {
    #[public]
    async fn open(&self, request: Request<String>) -> Result<Response<String>, Status> {
        Ok(Response::new(request.into_inner()))
    }
}

fn main() {}
