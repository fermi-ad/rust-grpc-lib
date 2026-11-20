// Without a policy the `#[async_trait]` order error still comes first: the
// methods have already been rewritten, so the real mistake is the order, not
// the missing policy.
use rust_grpc_lib::access_controlled_service;
use rust_grpc_lib::tonic::{Request, Response, Status};

#[rust_grpc_lib::async_trait]
trait Echo: Send + Sync + 'static {
    async fn echo(&self, request: Request<String>) -> Result<Response<String>, Status>;
}

struct Server;

#[rust_grpc_lib::async_trait]
#[access_controlled_service]
impl Echo for Server {
    async fn echo(&self, ctx: RequestContext<String>) -> Result<Response<String>, Status> {
        Ok(Response::new(ctx.message))
    }
}

fn main() {}
