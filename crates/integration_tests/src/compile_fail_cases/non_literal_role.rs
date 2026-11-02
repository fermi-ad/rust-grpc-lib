use rust_grpc_lib::keycloak_authenticated_service;
use tonic::{Request, Response, Status};

const ADMIN: &str = "admin";

trait Echo {
    fn echo(
        &self,
        request: Request<String>,
    ) -> impl Future<Output = Result<Response<String>, Status>> + Send;
}

struct Server;

#[keycloak_authenticated_service]
impl Echo for Server {
    #[roles(any(ADMIN))]
    async fn echo(&self, request: Request<String>) -> Result<Response<String>, Status> {
        Ok(Response::new(request.into_inner()))
    }
}

fn main() {}
