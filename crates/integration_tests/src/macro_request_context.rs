//! Tests for the `RequestContext` rewrite and the authorization-denial log of
//! `#[keycloak_authenticated_service]`.
//!
//! # What is tested
//!
//! - `RequestContext<T>` handlers (unary and streaming) get their context built
//!   from the incoming `Request<T>`, and the role guard reuses its claims.
//! - `Request<T>` and `RequestContext<T>` methods can be mixed in one `impl`,
//!   with the macro written above `#[async_trait]` or on a native `async fn`
//!   trait, and with `self: Arc<Self>` receivers.
//! - A missing claims extension fails closed with a generic `internal` status.
//! - A denied call logs one `warn` event on the `rust_grpc_lib::authz` target
//!   with `user`, `rpc.service`, `rpc.method` and `required_roles`, and never
//!   leaks the roles the caller holds. The client status is unchanged.

use std::io;
use std::sync::{Arc, Mutex};

use tonic::codec::{DecodeBuf, Decoder, Streaming};
use tonic::{Code, Request, Response, Status};
use tracing_subscriber::fmt::MakeWriter;

use rust_grpc_lib::auth::{Claims, KeycloakClaims, RequestContext};
use rust_grpc_lib::keycloak_authenticated_service;
use rust_grpc_lib::testing::{KeycloakClaimsBuilder, request_with_claims};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn make_claims(sub: &str, realm_roles: &[&str]) -> KeycloakClaims {
    realm_roles
        .iter()
        .fold(KeycloakClaimsBuilder::new(sub), |builder, role| {
            builder.realm_role(*role)
        })
        .build()
}

fn request_with<T>(message: T, claims: KeycloakClaims) -> Request<T> {
    request_with_claims(message, claims)
}

/// A decoder that never yields a message; enough to build a `Streaming` that
/// the handler under test does not poll.
struct EmptyDecoder;

impl Decoder for EmptyDecoder {
    type Item = String;
    type Error = Status;

    fn decode(&mut self, _src: &mut DecodeBuf<'_>) -> Result<Option<String>, Status> {
        Ok(None)
    }
}

fn empty_stream() -> Streaming<String> {
    Streaming::new_request(EmptyDecoder, tonic::body::Body::empty(), None, None)
}

/// Collects everything a `tracing` subscriber writes.
#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Capture {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

impl io::Write for Capture {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Capture {
    type Writer = Capture;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Installs a thread-local subscriber for the lifetime of the returned guard.
/// Tests using it must run on a current-thread runtime (the default for
/// `#[tokio::test]`).
fn capture_logs() -> (Capture, tracing::subscriber::DefaultGuard) {
    let capture = Capture::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(capture.clone())
        .with_ansi(false)
        .with_max_level(tracing::Level::TRACE)
        .finish();
    let guard = tracing::subscriber::set_default(subscriber);
    (capture, guard)
}

// ---------------------------------------------------------------------------
// Service under test: native async fn trait, mixed parameter forms
// ---------------------------------------------------------------------------

trait ContextService {
    async fn unary(&self, request: Request<String>) -> Result<Response<String>, Status>;
    async fn any_roles(&self, request: Request<String>) -> Result<Response<String>, Status>;
    async fn all_roles(&self, request: Request<String>) -> Result<Response<String>, Status>;
    async fn open(&self, request: Request<String>) -> Result<Response<String>, Status>;
    async fn plain(&self, request: Request<String>) -> Result<Response<String>, Status>;
    async fn consume(
        &self,
        request: Request<Streaming<String>>,
    ) -> Result<Response<String>, Status>;
    async fn consume_open(
        &self,
        request: Request<Streaming<String>>,
    ) -> Result<Response<String>, Status>;
}

struct ContextServer;

#[keycloak_authenticated_service(claims = rust_grpc_lib::auth::KeycloakClaims)]
impl ContextService for ContextServer {
    /// `Request<T>` form with a role guard.
    #[roles(any("admin"))]
    async fn unary(&self, request: Request<String>) -> Result<Response<String>, Status> {
        Ok(Response::new(format!("unary: {}", request.into_inner())))
    }

    /// `RequestContext<T>` form, `any(..)` guard.
    #[roles(any("admin", "operator"))]
    async fn any_roles(&self, ctx: RequestContext<String>) -> Result<Response<String>, Status> {
        Ok(Response::new(format!(
            "any: {} by {} ({} bytes of metadata)",
            ctx.message,
            ctx.sub(),
            ctx.metadata.len()
        )))
    }

    /// `RequestContext<T>` form, `all(..)` guard.
    #[roles(all("admin", "operator"))]
    async fn all_roles(&self, ctx: RequestContext<String>) -> Result<Response<String>, Status> {
        Ok(Response::new(format!("all: {}", ctx.message)))
    }

    /// `RequestContext<T>` form without a guard: only the context is built.
    async fn open(&self, mut ctx: RequestContext<String>) -> Result<Response<String>, Status> {
        ctx.message.push('!');
        Ok(Response::new(ctx.message))
    }

    /// Untouched `Request<T>` method without a guard.
    async fn plain(&self, request: Request<String>) -> Result<Response<String>, Status> {
        Ok(Response::new(request.into_inner()))
    }

    /// Streaming request with a role guard.
    #[roles(any("admin"))]
    async fn consume(
        &self,
        ctx: RequestContext<Streaming<String>>,
    ) -> Result<Response<String>, Status> {
        let _stream: Streaming<String> = ctx.message;
        Ok(Response::new(format!("consumed for {}", ctx.claims.sub())))
    }

    /// Streaming request without a guard.
    async fn consume_open(
        &self,
        ctx: RequestContext<Streaming<String>>,
    ) -> Result<Response<String>, Status> {
        Ok(Response::new(format!("open for {}", ctx.sub())))
    }
}

// ---------------------------------------------------------------------------
// Context rewrite
// ---------------------------------------------------------------------------

#[tokio::test]
async fn context_handler_receives_message_claims_and_metadata() {
    let mut request = request_with("hi".to_string(), make_claims("alice", &["operator"]));
    request
        .metadata_mut()
        .insert("x-test", "1".parse().unwrap());

    let response = ContextServer.any_roles(request).await.unwrap();

    assert_eq!(
        response.into_inner(),
        "any: hi by alice (1 bytes of metadata)"
    );
}

#[tokio::test]
async fn context_handler_without_roles_still_gets_a_context() {
    let request = request_with("hey".to_string(), make_claims("alice", &[]));

    let response = ContextServer.open(request).await.unwrap();

    assert_eq!(response.into_inner(), "hey!");
}

#[tokio::test]
async fn context_handler_without_claims_returns_generic_internal() {
    let err = ContextServer
        .open(Request::new("hey".to_string()))
        .await
        .unwrap_err();

    assert_eq!(err.code(), Code::Internal);
    assert_eq!(err.message(), "authentication is not configured");
}

#[tokio::test]
async fn request_handler_without_claims_returns_generic_internal() {
    let err = ContextServer
        .unary(Request::new("hey".to_string()))
        .await
        .unwrap_err();

    assert_eq!(err.code(), Code::Internal);
    assert_eq!(err.message(), "authentication is not configured");
}

#[tokio::test]
async fn untouched_request_method_still_works_without_claims() {
    let response = ContextServer
        .plain(Request::new("plain".to_string()))
        .await
        .unwrap();

    assert_eq!(response.into_inner(), "plain");
}

#[tokio::test]
async fn mixed_forms_share_one_impl() {
    let claims = || make_claims("bob", &["admin", "operator"]);

    let unary = ContextServer
        .unary(request_with("a".to_string(), claims()))
        .await
        .unwrap();
    let all = ContextServer
        .all_roles(request_with("b".to_string(), claims()))
        .await
        .unwrap();

    assert_eq!(unary.into_inner(), "unary: a");
    assert_eq!(all.into_inner(), "all: b");
}

// ---------------------------------------------------------------------------
// Role guard through the context
// ---------------------------------------------------------------------------

#[tokio::test]
async fn context_any_guard_denies_without_a_listed_role() {
    let request = request_with("x".to_string(), make_claims("carol", &["viewer"]));

    let err = ContextServer.any_roles(request).await.unwrap_err();

    assert_eq!(err.code(), Code::PermissionDenied);
    assert_eq!(err.message(), "required role not present");
}

#[tokio::test]
async fn context_all_guard_denies_on_partial_match() {
    let request = request_with("x".to_string(), make_claims("carol", &["admin"]));

    let err = ContextServer.all_roles(request).await.unwrap_err();

    assert_eq!(err.code(), Code::PermissionDenied);
}

// ---------------------------------------------------------------------------
// Streaming handlers
// ---------------------------------------------------------------------------

#[tokio::test]
async fn streaming_context_handler_runs_when_permitted() {
    let request = request_with(empty_stream(), make_claims("dave", &["admin"]));

    let response = ContextServer.consume(request).await.unwrap();

    assert_eq!(response.into_inner(), "consumed for dave");
}

#[tokio::test]
async fn streaming_context_handler_is_denied_without_role() {
    let request = request_with(empty_stream(), make_claims("dave", &["viewer"]));

    let err = ContextServer.consume(request).await.unwrap_err();

    assert_eq!(err.code(), Code::PermissionDenied);
}

#[tokio::test]
async fn streaming_context_handler_without_guard_gets_context() {
    let request = request_with(empty_stream(), make_claims("erin", &[]));

    let response = ContextServer.consume_open(request).await.unwrap();

    assert_eq!(response.into_inner(), "open for erin");
}

// ---------------------------------------------------------------------------
// `self: Arc<Self>` receivers
// ---------------------------------------------------------------------------

trait ArcService {
    async fn shared(self: Arc<Self>, request: Request<String>) -> Result<Response<String>, Status>;
}

struct ArcServer;

#[keycloak_authenticated_service]
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
async fn arc_self_receiver_with_context_is_supported() {
    let server = Arc::new(ArcServer);

    let ok = server
        .clone()
        .shared(request_with("m".to_string(), make_claims("f", &["admin"])))
        .await
        .unwrap();
    let denied = server
        .shared(request_with("m".to_string(), make_claims("f", &[])))
        .await
        .unwrap_err();

    assert_eq!(ok.into_inner(), "arc: m");
    assert_eq!(denied.code(), Code::PermissionDenied);
}

// ---------------------------------------------------------------------------
// `#[async_trait]` below the macro
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
trait BoxedService: Send + Sync + 'static {
    async fn boxed_ctx(&self, request: Request<String>) -> Result<Response<String>, Status>;
    async fn boxed_req(&self, request: Request<String>) -> Result<Response<String>, Status>;
}

struct BoxedServer;

#[keycloak_authenticated_service]
#[async_trait::async_trait]
impl BoxedService for BoxedServer {
    #[roles(all("admin"))]
    async fn boxed_ctx(&self, ctx: RequestContext<String>) -> Result<Response<String>, Status> {
        Ok(Response::new(format!("ctx: {}", ctx.message)))
    }

    #[roles(any("admin"))]
    async fn boxed_req(&self, request: Request<String>) -> Result<Response<String>, Status> {
        Ok(Response::new(format!("req: {}", request.into_inner())))
    }
}

#[tokio::test]
async fn macro_above_async_trait_handles_both_forms() {
    let svc: Box<dyn BoxedService> = Box::new(BoxedServer);
    let claims = || make_claims("g", &["admin"]);

    let ctx = svc
        .boxed_ctx(request_with("1".to_string(), claims()))
        .await
        .unwrap();
    let req = svc
        .boxed_req(request_with("2".to_string(), claims()))
        .await
        .unwrap();
    let denied = svc
        .boxed_ctx(request_with("3".to_string(), make_claims("g", &[])))
        .await
        .unwrap_err();

    assert_eq!(ctx.into_inner(), "ctx: 1");
    assert_eq!(req.into_inner(), "req: 2");
    assert_eq!(denied.code(), Code::PermissionDenied);
}

// ---------------------------------------------------------------------------
// Authorization-denial logging
// ---------------------------------------------------------------------------

#[tokio::test]
async fn denied_request_form_logs_authz_warning() {
    let (logs, _guard) = capture_logs();
    let request = request_with(
        "x".to_string(),
        make_claims("mallory", &["secret-held-role"]),
    );

    let err = ContextServer.unary(request).await.unwrap_err();

    assert_eq!(err.code(), Code::PermissionDenied);
    assert_eq!(err.message(), "required role not present");
    let text = logs.text();
    assert!(text.contains("WARN"), "{text}");
    assert!(text.contains("rust_grpc_lib::authz"), "{text}");
    assert!(text.contains("authorization denied"), "{text}");
    assert!(text.contains(r#"user="mallory""#), "{text}");
    assert!(text.contains(r#"rpc.service="ContextService""#), "{text}");
    assert!(text.contains(r#"rpc.method="unary""#), "{text}");
    assert!(text.contains(r#"required_roles=["admin"]"#), "{text}");
    assert!(text.contains(r#"role_mode="any""#), "{text}");
    assert!(
        !text.contains("secret-held-role"),
        "held roles leaked: {text}"
    );
}

#[tokio::test]
async fn denied_context_form_logs_any_mode_with_all_required_roles() {
    let (logs, _guard) = capture_logs();
    let request = request_with("x".to_string(), make_claims("mallory", &["viewer"]));

    ContextServer.any_roles(request).await.unwrap_err();

    let text = logs.text();
    assert!(text.contains(r#"user="mallory""#), "{text}");
    assert!(text.contains(r#"rpc.method="any_roles""#), "{text}");
    assert!(
        text.contains(r#"required_roles=["admin", "operator"]"#),
        "{text}"
    );
    assert!(text.contains(r#"role_mode="any""#), "{text}");
    assert!(!text.contains("viewer"), "held roles leaked: {text}");
}

#[tokio::test]
async fn denied_all_mode_is_logged_as_all() {
    let (logs, _guard) = capture_logs();
    let request = request_with("x".to_string(), make_claims("mallory", &["admin"]));

    ContextServer.all_roles(request).await.unwrap_err();

    let text = logs.text();
    assert!(text.contains(r#"role_mode="all""#), "{text}");
    assert!(
        text.contains(r#"required_roles=["admin", "operator"]"#),
        "{text}"
    );
}

#[tokio::test]
async fn denied_streaming_handler_logs_authz_warning() {
    let (logs, _guard) = capture_logs();
    let request = request_with(empty_stream(), make_claims("mallory", &["viewer"]));

    ContextServer.consume(request).await.unwrap_err();

    let text = logs.text();
    assert!(text.contains("authorization denied"), "{text}");
    assert!(text.contains(r#"rpc.method="consume""#), "{text}");
    assert!(text.contains(r#"user="mallory""#), "{text}");
}

#[tokio::test]
async fn permitted_call_logs_no_denial() {
    let (logs, _guard) = capture_logs();

    ContextServer
        .any_roles(request_with("x".to_string(), make_claims("ok", &["admin"])))
        .await
        .unwrap();
    ContextServer
        .unary(request_with("x".to_string(), make_claims("ok", &["admin"])))
        .await
        .unwrap();

    let text = logs.text();
    assert!(!text.contains("authorization denied"), "{text}");
    assert!(!text.contains("rust_grpc_lib::authz"), "{text}");
}

#[tokio::test]
async fn missing_claims_is_an_error_event_not_an_authz_denial() {
    let (logs, _guard) = capture_logs();

    ContextServer
        .unary(Request::new("x".to_string()))
        .await
        .unwrap_err();

    let text = logs.text();
    assert!(text.contains("ERROR"), "{text}");
    assert!(text.contains("no claims extension"), "{text}");
    assert!(!text.contains("authorization denied"), "{text}");
    assert!(!text.contains("rust_grpc_lib::authz"), "{text}");
}
