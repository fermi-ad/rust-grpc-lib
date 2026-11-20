// A `#[public]` method has no claims to give an `AuthenticatedContext`; it takes a
// `PublicContext`.
use rust_grpc_lib::access_controlled_service;
use rust_grpc_lib::tonic::{Request, Response, Status};

trait Echo {
    fn echo(
        &self,
        request: Request<String>,
    ) -> impl Future<Output = Result<Response<String>, Status>> + Send;
}

struct Server;

#[access_controlled_service]
impl Echo for Server {
    #[public]
    async fn echo(
        &self,
        ctx: rust_grpc_lib::auth::AuthenticatedContext<String>,
    ) -> Result<Response<String>, Status> {
        Ok(Response::new(ctx.message))
    }
}

fn main() {}
