// Every method must declare an access policy; the error names each method
// that does not.
use rust_grpc_lib::access_controlled_service;
use rust_grpc_lib::tonic::{Request, Response, Status};

trait Echo {
    fn echo(
        &self,
        request: Request<String>,
    ) -> impl Future<Output = Result<Response<String>, Status>> + Send;

    fn shout(
        &self,
        request: Request<String>,
    ) -> impl Future<Output = Result<Response<String>, Status>> + Send;

    fn whisper(
        &self,
        request: Request<String>,
    ) -> impl Future<Output = Result<Response<String>, Status>> + Send;
}

struct Server;

#[access_controlled_service]
impl Echo for Server {
    #[roles(any("admin"))]
    async fn echo(&self, ctx: RequestContext<String>) -> Result<Response<String>, Status> {
        Ok(Response::new(ctx.message))
    }

    async fn shout(&self, ctx: RequestContext<String>) -> Result<Response<String>, Status> {
        Ok(Response::new(ctx.message))
    }

    async fn whisper(&self, ctx: RequestContext<String>) -> Result<Response<String>, Status> {
        Ok(Response::new(ctx.message))
    }
}

fn main() {}
