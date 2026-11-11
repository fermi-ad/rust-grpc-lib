// The gate matches proto method names against Rust method names after
// lowercasing and removing underscores, so two methods that normalize to the
// same name could not be told apart.
use rust_grpc_lib::access_controlled_service;
use rust_grpc_lib::tonic::{Request, Response, Status};

trait Echo {
    fn get_user(
        &self,
        request: Request<String>,
    ) -> impl Future<Output = Result<Response<String>, Status>> + Send;

    fn getuser(
        &self,
        request: Request<String>,
    ) -> impl Future<Output = Result<Response<String>, Status>> + Send;
}

struct Server;

#[access_controlled_service]
impl Echo for Server {
    #[public]
    async fn get_user(&self, ctx: rust_grpc_lib::auth::PublicContext<String>) -> Result<Response<String>, Status> {
        Ok(Response::new(ctx.message))
    }

    #[public]
    async fn getuser(&self, ctx: rust_grpc_lib::auth::PublicContext<String>) -> Result<Response<String>, Status> {
        Ok(Response::new(ctx.message))
    }
}

fn main() {}
