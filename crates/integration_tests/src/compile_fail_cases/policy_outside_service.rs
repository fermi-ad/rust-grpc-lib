// A policy attribute only means something inside an `impl` block annotated
// with `#[access_controlled_service]`. Anywhere else it would silently
// skip the check, so it is an error.
use rust_grpc_lib::{any_authenticated, roles};
use rust_grpc_lib::tonic::{Request, Response, Status};

struct Server;

impl Server {
    #[roles(any("admin"))]
    async fn guarded(&self, ctx: RequestContext<String>) -> Result<Response<String>, Status> {
        Ok(Response::new(ctx.message))
    }

    #[any_authenticated]
    async fn authenticated(&self, ctx: RequestContext<String>) -> Result<Response<String>, Status> {
        Ok(Response::new(ctx.message))
    }
}

fn main() {}
