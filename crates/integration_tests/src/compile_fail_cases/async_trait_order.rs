// `#[async_trait]` below the macro is required. Written the other way round,
// the macro sees methods that async_trait has already rewritten.
use rust_grpc_lib::keycloak_authenticated_service;
use tonic::{Request, Response, Status};

#[async_trait::async_trait]
trait Echo: Send + Sync + 'static {
    async fn echo(&self, request: Request<String>) -> Result<Response<String>, Status>;
}

struct Server;

#[async_trait::async_trait]
#[keycloak_authenticated_service]
impl Echo for Server {
    #[roles(any("admin"))]
    async fn echo(&self, request: Request<String>) -> Result<Response<String>, Status> {
        Ok(Response::new(request.into_inner()))
    }
}

fn main() {}
