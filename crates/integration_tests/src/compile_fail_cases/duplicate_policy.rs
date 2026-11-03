// A method declares exactly one access policy.
use rust_grpc_lib::keycloak_authenticated_service;
use tonic::{Request, Response, Status};

trait Echo {
    fn echo(
        &self,
        request: Request<String>,
    ) -> impl Future<Output = Result<Response<String>, Status>> + Send;
}

struct Server;

#[keycloak_authenticated_service(strict)]
impl Echo for Server {
    #[any_authenticated]
    #[roles(any("admin"))]
    async fn echo(&self, request: Request<String>) -> Result<Response<String>, Status> {
        Ok(Response::new(request.into_inner()))
    }
}

fn main() {}
