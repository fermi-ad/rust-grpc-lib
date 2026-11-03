// In a `strict` block the `#[async_trait]` order error still comes first: the
// methods have already been rewritten, so the real mistake is the order, not
// the missing policy.
use rust_grpc_lib::keycloak_authenticated_service;
use tonic::{Request, Response, Status};

#[async_trait::async_trait]
trait Echo: Send + Sync + 'static {
    async fn echo(&self, request: Request<String>) -> Result<Response<String>, Status>;
}

struct Server;

#[async_trait::async_trait]
#[keycloak_authenticated_service(strict)]
impl Echo for Server {
    async fn echo(&self, request: Request<String>) -> Result<Response<String>, Status> {
        Ok(Response::new(request.into_inner()))
    }
}

fn main() {}
