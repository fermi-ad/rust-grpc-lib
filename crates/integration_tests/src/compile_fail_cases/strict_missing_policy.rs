// In a `strict` block every method must declare an access policy; the error
// names each method that does not.
use rust_grpc_lib::keycloak_authenticated_service;
use tonic::{Request, Response, Status};

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

#[keycloak_authenticated_service(strict)]
impl Echo for Server {
    #[roles(any("admin"))]
    async fn echo(&self, request: Request<String>) -> Result<Response<String>, Status> {
        Ok(Response::new(request.into_inner()))
    }

    async fn shout(&self, request: Request<String>) -> Result<Response<String>, Status> {
        Ok(Response::new(request.into_inner()))
    }

    async fn whisper(&self, request: Request<String>) -> Result<Response<String>, Status> {
        Ok(Response::new(request.into_inner()))
    }
}

fn main() {}
