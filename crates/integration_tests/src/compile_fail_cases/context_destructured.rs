use rust_grpc_lib::keycloak_authenticated_service;
use tonic::{Request, Response, Status};

trait Echo {
    fn echo(
        &self,
        request: Request<String>,
    ) -> impl Future<Output = Result<Response<String>, Status>> + Send;
}

struct Server;

#[keycloak_authenticated_service]
impl Echo for Server {
    async fn echo(
        &self,
        (a, b): rust_grpc_lib::auth::RequestContext<String>,
    ) -> Result<Response<String>, Status> {
        Ok(Response::new(String::new()))
    }
}

fn main() {}
