//! `#[access_controlled_service]` expansion without a server: the emitted
//! [`ServicePolicy`] tables and handler context rewriting, with handlers
//! called directly. Enforcement is in `access_gate`.

use std::marker::PhantomData;
use std::sync::Arc;

use rust_grpc_lib::access_controlled_service;
use rust_grpc_lib::auth::{Access, PolicyTable, PublicContext, RequestContext};
use rust_grpc_lib::testing::{authenticated_request, claims_with_roles, public_request};
use rust_grpc_lib::tonic::body::Body;
use rust_grpc_lib::tonic::codec::{DecodeBuf, Decoder, Streaming};
use rust_grpc_lib::tonic::{Code, Request, Response, Status};

/// Role constants, for `#[roles(..)]` with paths instead of literals.
mod roles {
    pub const OPERATOR: &str = "operator";
    pub const ADMIN: &str = "admin";
}

#[allow(dead_code)]
trait EchoService {
    async fn any_role(&self, request: Request<String>) -> Result<Response<String>, Status>;
    async fn all_roles(&self, request: Request<String>) -> Result<Response<String>, Status>;
    async fn typed(&self, request: Request<String>) -> Result<Response<String>, Status>;
    async fn signed_in(&self, request: Request<String>) -> Result<Response<String>, Status>;
    async fn open(&self, request: Request<String>) -> Result<Response<String>, Status>;
    async fn consume(
        &self,
        request: Request<Streaming<String>>,
    ) -> Result<Response<String>, Status>;
    async fn consume_open(
        &self,
        request: Request<Streaming<String>>,
    ) -> Result<Response<String>, Status>;
}

struct EchoServer;

// The macro only accepts `async fn` handlers, so this lint cannot be fixed.
#[allow(clippy::unused_async_trait_impl)]
#[access_controlled_service(server = EchoServerStandIn)]
impl EchoService for EchoServer {
    #[roles(any("admin", "operator"))]
    async fn any_role(&self, ctx: RequestContext<String>) -> Result<Response<String>, Status> {
        Ok(Response::new(format!(
            "any: {} by {} at {}",
            ctx.message,
            ctx.sub(),
            ctx.rpc.method
        )))
    }

    #[roles(all("admin", "operator"))]
    async fn all_roles(&self, ctx: RequestContext<String>) -> Result<Response<String>, Status> {
        Ok(Response::new(format!("all: {}", ctx.message)))
    }

    #[roles(any(roles::OPERATOR, "viewer"))]
    async fn typed(&self, ctx: RequestContext<String>) -> Result<Response<String>, Status> {
        Ok(Response::new(ctx.message))
    }

    #[any_authenticated]
    async fn signed_in(&self, mut ctx: RequestContext<String>) -> Result<Response<String>, Status> {
        ctx.message.push('!');
        Ok(Response::new(ctx.message))
    }

    #[public]
    async fn open(&self, ctx: PublicContext<String>) -> Result<Response<String>, Status> {
        Ok(Response::new(format!("open: {}", ctx.message)))
    }

    #[roles(all(roles::ADMIN))]
    async fn consume(
        &self,
        ctx: RequestContext<Streaming<String>>,
    ) -> Result<Response<String>, Status> {
        let who = ctx.sub().to_owned();
        let _stream: Streaming<String> = ctx.message;
        Ok(Response::new(format!("consumed for {who}")))
    }

    #[public]
    async fn consume_open(
        &self,
        ctx: PublicContext<Streaming<String>>,
    ) -> Result<Response<String>, Status> {
        let _stream: Streaming<String> = ctx.message;
        Ok(Response::new("open stream".to_owned()))
    }
}

/// Stands in for a generated `FooServer<T>`, which keys the table; any
/// one-parameter generic type works.
struct EchoServerStandIn<T>(#[allow(dead_code)] T);

fn table() -> PolicyTable {
    PolicyTable::for_service(EchoServerStandIn::<EchoServer>)
}

/// A request as the gate leaves it after admitting `sub`.
fn authenticated<T>(message: T, sub: &str, held: &[&str]) -> Request<T> {
    authenticated_request(message, claims_with_roles(sub, held))
}

struct EmptyDecoder;

impl Decoder for EmptyDecoder {
    type Item = String;
    type Error = Status;

    fn decode(&mut self, _src: &mut DecodeBuf<'_>) -> Result<Option<String>, Status> {
        Ok(None)
    }
}

fn empty_stream() -> Streaming<String> {
    Streaming::new_request(EmptyDecoder, Body::empty(), None, None)
}

#[test]
fn the_table_lists_every_method_with_the_policy_it_declared() {
    let table = table();

    assert_eq!(
        table.lookup("any_role"),
        Some(Access::any_of(&["admin", "operator"]))
    );
    assert_eq!(
        table.lookup("all_roles"),
        Some(Access::all_of(&["admin", "operator"]))
    );
    assert_eq!(
        table.lookup("typed"),
        Some(Access::any_of(&[roles::OPERATOR, "viewer"]))
    );
    assert_eq!(table.lookup("signed_in"), Some(Access::Authenticated));
    assert_eq!(table.lookup("open"), Some(Access::Public));
    assert_eq!(
        table.lookup("consume"),
        Some(Access::all_of(&[roles::ADMIN]))
    );
    assert_eq!(table.lookup("consume_open"), Some(Access::Public));
    assert_eq!(table.policies().count(), 7);
}

#[tokio::test]
async fn streaming_contexts_are_supported() {
    let authenticated_stream = authenticated(empty_stream(), "alice", &["admin"]);

    let response = EchoServer.consume(authenticated_stream).await.unwrap();
    let open = EchoServer
        .consume_open(public_request(empty_stream()))
        .await
        .unwrap();

    assert_eq!(response.into_inner(), "consumed for alice");
    assert_eq!(open.into_inner(), "open stream");
}

#[tokio::test]
async fn a_handler_called_without_the_gate_fails_closed_with_a_generic_message() {
    let ungated = Request::new("x".to_string());

    let err = EchoServer.any_role(ungated).await.unwrap_err();

    assert_eq!(err.code(), Code::Internal);
    assert_eq!(err.message(), "authentication is not configured");
}

trait ArcService {
    async fn shared(self: Arc<Self>, request: Request<String>) -> Result<Response<String>, Status>;
}

struct ArcServer;

struct ArcServerStandIn<T>(PhantomData<T>);

#[allow(clippy::unused_async_trait_impl)]
#[access_controlled_service(server = ArcServerStandIn)]
impl ArcService for ArcServer {
    #[roles(any("admin"))]
    async fn shared(
        self: Arc<Self>,
        ctx: RequestContext<String>,
    ) -> Result<Response<String>, Status> {
        Ok(Response::new(format!("arc: {}", ctx.message)))
    }
}

#[tokio::test]
async fn an_arc_self_receiver_with_a_context_is_supported() {
    let server = Arc::new(ArcServer);

    let response = server
        .shared(authenticated("m".to_string(), "f", &["admin"]))
        .await
        .unwrap();

    assert_eq!(response.into_inner(), "arc: m");
}

#[rust_grpc_lib::async_trait]
trait BoxedService: Send + Sync + 'static {
    async fn boxed_ctx(&self, request: Request<String>) -> Result<Response<String>, Status>;
    async fn boxed_open(&self, request: Request<String>) -> Result<Response<String>, Status>;
}

struct BoxedServer;

struct BoxedServerStandIn<T>(PhantomData<T>);

#[access_controlled_service(server = BoxedServerStandIn)]
#[rust_grpc_lib::async_trait]
impl BoxedService for BoxedServer {
    #[roles(all("admin"))]
    async fn boxed_ctx(&self, ctx: RequestContext<String>) -> Result<Response<String>, Status> {
        Ok(Response::new(format!("ctx: {}", ctx.message)))
    }

    #[public]
    async fn boxed_open(&self, ctx: PublicContext<String>) -> Result<Response<String>, Status> {
        Ok(Response::new(format!("open: {}", ctx.message)))
    }
}

#[tokio::test]
async fn the_macro_above_async_trait_handles_both_contexts() {
    let svc: Box<dyn BoxedService> = Box::new(BoxedServer);

    let ctx = svc
        .boxed_ctx(authenticated("1".to_string(), "g", &["admin"]))
        .await
        .unwrap();
    let open = svc
        .boxed_open(public_request("2".to_string()))
        .await
        .unwrap();

    assert_eq!(ctx.into_inner(), "ctx: 1");
    assert_eq!(open.into_inner(), "open: 2");
}

#[allow(dead_code)]
trait First {
    async fn same(&self, request: Request<String>) -> Result<Response<String>, Status>;
}

#[allow(dead_code)]
trait Second {
    async fn same(&self, request: Request<String>) -> Result<Response<String>, Status>;
}

struct Both;
struct FirstServer<T>(#[allow(dead_code)] T);
struct SecondServer<T>(#[allow(dead_code)] T);

#[allow(clippy::unused_async_trait_impl)]
#[access_controlled_service]
impl First for Both {
    #[public]
    async fn same(&self, ctx: PublicContext<String>) -> Result<Response<String>, Status> {
        Ok(Response::new(ctx.message))
    }
}

#[allow(clippy::unused_async_trait_impl)]
#[access_controlled_service]
impl Second for Both {
    #[roles(all("admin"))]
    async fn same(&self, ctx: RequestContext<String>) -> Result<Response<String>, Status> {
        Ok(Response::new(ctx.message))
    }
}

#[test]
fn a_method_name_shared_by_two_services_has_a_table_for_each() {
    let first = PolicyTable::for_service(FirstServer::<Both>);
    let second = PolicyTable::for_service(SecondServer::<Both>);

    assert_eq!(first.lookup("same"), Some(Access::Public));
    assert_eq!(second.lookup("same"), Some(Access::all_of(&["admin"])));
}
