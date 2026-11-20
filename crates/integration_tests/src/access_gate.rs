//! The access gate on live servers: policy enforcement and status codes,
//! fail-closed handling, token scrubbing, spans and denial logs, the
//! conformance helpers and token forwarding. Semantics: `rust_grpc_lib::auth`.
//!
//! Span/log tests use the current-thread runtime so the thread-local
//! subscriber sees the server task.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use rust_grpc_lib::access_controlled_service;
use rust_grpc_lib::auth::{
    Access, CallInfo, MethodPolicy, PublicContext, RequestContext, RpcMethod, ServicePolicy,
};
use rust_grpc_lib::server::GrpcServer;
use rust_grpc_lib::testing::{
    assert_policy_table_conforms, assert_service_conforms, capture_spans, expired_test_token,
    test_token,
};
use rust_grpc_lib::tonic::metadata::MetadataMap;
use rust_grpc_lib::tonic::{Code, Request, Response, Status};

use crate::services::example::{
    IdList, PartOne, PartThree, ReplyOne, ReplyTwo, SectionOne, SectionTwo,
    example_db_client::ExampleDbClient,
    example_db_server::{ExampleDb, ExampleDbServer, SERVICE_NAME},
};
use crate::support::{
    Running, TEST_KID, TEST_USER, capture_logs, client, connect, make_jwt, make_validator,
    rpc_span, serve,
};

/// What a handler saw.
#[derive(Debug)]
struct Seen {
    rpc: RpcMethod,
    authorization_in_metadata: bool,
    /// The context's `Debug` output.
    debug: String,
    /// `None` for public methods.
    user: Option<String>,
}

#[derive(Default)]
struct Calls(Mutex<Vec<Seen>>);

impl Calls {
    fn record(&self, info: &CallInfo, debug: String, user: Option<&str>) {
        self.0.lock().unwrap().push(Seen {
            rpc: info.rpc.clone(),
            authorization_in_metadata: has_authorization(&info.metadata),
            debug,
            user: user.map(str::to_owned),
        });
    }

    fn count(&self) -> usize {
        self.0.lock().unwrap().len()
    }
}

fn has_authorization(metadata: &MetadataMap) -> bool {
    metadata.contains_key("authorization")
}

fn reply_one(sub: &str) -> ReplyOne {
    ReplyOne {
        section_one: vec![SectionOne {
            id: 1,
            part_one: Some(PartOne {
                some_string_alpha: sub.to_owned(),
                some_integer_alpha: 0,
            }),
            part_two: None,
            part_three: None,
        }],
    }
}

fn reply_two(sub: &str) -> ReplyTwo {
    ReplyTwo {
        section_two: vec![SectionTwo {
            id: 2,
            part_three: Some(PartThree {
                some_string_delta: sub.to_owned(),
                some_bool_delta: true,
                some_integer_delta: None,
            }),
        }],
    }
}

/// `getEndPointOne` needs the `operator` role; `getEndPointTwo` is public.
struct Gated {
    calls: Arc<Calls>,
}

#[access_controlled_service]
#[rust_grpc_lib::async_trait]
impl ExampleDb for Gated {
    #[roles(any("operator"))]
    async fn get_end_point_one(
        &self,
        ctx: RequestContext<IdList>,
    ) -> Result<Response<ReplyOne>, Status> {
        self.calls.record(&ctx, format!("{ctx:?}"), Some(ctx.sub()));
        Ok(Response::new(reply_one(ctx.sub())))
    }

    #[public]
    async fn get_end_point_two(
        &self,
        ctx: PublicContext<IdList>,
    ) -> Result<Response<ReplyTwo>, Status> {
        self.calls.record(&ctx, format!("{ctx:?}"), None);
        Ok(Response::new(reply_two("nobody")))
    }
}

async fn start_gated() -> (Running, Arc<Calls>) {
    let calls = Arc::new(Calls::default());
    let running = serve(GrpcServer::builder(make_validator()).add_service(
        ExampleDbServer::new,
        Gated {
            calls: Arc::clone(&calls),
        },
    ))
    .await;
    (running, calls)
}

fn ids() -> IdList {
    IdList { id: vec![1] }
}

#[tokio::test]
async fn a_public_method_is_reachable_without_a_token_and_still_has_a_method_and_a_span() {
    let (capture, _guard) = capture_spans();
    let (running, calls) = start_gated().await;

    client(running.addr, "")
        .get_end_point_two(ids())
        .await
        .expect("a public method needs no token");

    let seen = &calls.0.lock().unwrap()[0];
    assert_eq!(seen.rpc.service, SERVICE_NAME);
    assert_eq!(seen.rpc.method, "getEndPointTwo");
    let span = rpc_span(&capture, "getEndPointTwo");
    assert_eq!(span.field("access"), Some("public"));
    assert_eq!(span.field("grpc.status"), Some("Ok"));
    assert!(!span.has_field("user"), "nobody is authenticated");
}

#[tokio::test]
async fn a_public_method_never_sees_a_token_the_caller_sent() {
    let (running, calls) = start_gated().await;
    let token = make_jwt(&["operator"]);

    client(running.addr, &token)
        .get_end_point_two(ids())
        .await
        .expect("a token sent to a public method is ignored, not rejected");

    let seen = &calls.0.lock().unwrap()[0];
    assert!(!seen.authorization_in_metadata);
    assert!(!seen.debug.contains(&token), "{}", seen.debug);
    assert!(seen.user.is_none());
}

#[tokio::test]
async fn a_caller_with_the_role_reaches_the_handler_with_claims_and_without_the_token_in_metadata()
{
    let (running, calls) = start_gated().await;
    let token = test_token(TEST_KID, "alice", &["operator"]);

    let reply = client(running.addr, &token)
        .get_end_point_one(ids())
        .await
        .expect("the operator is admitted")
        .into_inner();

    assert_eq!(
        reply.section_one[0]
            .part_one
            .as_ref()
            .unwrap()
            .some_string_alpha,
        "alice"
    );
    let seen = &calls.0.lock().unwrap()[0];
    assert_eq!(seen.user.as_deref(), Some("alice"));
    assert!(!seen.authorization_in_metadata);
    assert!(!seen.debug.contains(&token), "{}", seen.debug);
    assert!(seen.debug.contains("redacted"), "{}", seen.debug);
}

#[tokio::test]
async fn every_way_of_not_authenticating_is_unauthenticated_and_never_enters_the_handler() {
    let (running, calls) = start_gated().await;
    let expired = expired_test_token(TEST_KID, TEST_USER, &["operator"]);

    for (label, token) in [
        ("no token", String::new()),
        ("malformed token", "not-a-jwt".to_owned()),
        ("expired token", expired),
    ] {
        let err = client(running.addr, &token)
            .get_end_point_one(ids())
            .await
            .unwrap_err();

        assert_eq!(err.code(), Code::Unauthenticated, "{label}");
    }
    assert_eq!(calls.count(), 0);
}

#[tokio::test]
async fn a_denial_logs_the_required_roles_and_never_the_held_ones_or_the_token() {
    let (logs, _guard) = capture_logs();
    let (running, _calls) = start_gated().await;
    let token = test_token(TEST_KID, "mallory", &["secret-held-role"]);

    client(running.addr, &token)
        .get_end_point_one(ids())
        .await
        .unwrap_err();

    let text = logs.text();
    assert!(text.contains("WARN"), "{text}");
    assert!(text.contains("rust_grpc_lib::authz"), "{text}");
    assert!(text.contains("authorization denied"), "{text}");
    assert!(text.contains(r#"user="mallory""#), "{text}");
    assert!(
        text.contains(&format!(r#"rpc.service="{SERVICE_NAME}""#)),
        "{text}"
    );
    assert!(text.contains(r#"rpc.method="getEndPointOne""#), "{text}");
    assert!(text.contains(r#"required_roles=["operator"]"#), "{text}");
    assert!(text.contains(r#"role_mode="any""#), "{text}");
    assert!(text.contains("remote_addr="), "{text}");
    assert!(
        !text.contains("secret-held-role"),
        "held roles leaked: {text}"
    );
    assert!(!text.contains(&token), "token leaked: {text}");
}

#[tokio::test]
async fn access_and_user_are_on_the_span_for_every_outcome() {
    let (capture, _guard) = capture_spans();
    let (running, _calls) = start_gated().await;

    // Spans are read by index (three share `getEndPointOne`), so keep these
    // calls sequential and in order: public, admitted, denied, no token.
    client(running.addr, "")
        .get_end_point_two(ids())
        .await
        .unwrap();
    client(running.addr, &make_jwt(&["operator"]))
        .get_end_point_one(ids())
        .await
        .unwrap();
    client(running.addr, &make_jwt(&["viewer"]))
        .get_end_point_one(ids())
        .await
        .unwrap_err();
    client(running.addr, "")
        .get_end_point_one(ids())
        .await
        .unwrap_err();

    let spans = capture.spans("rpc");
    let admitted = &spans[1];
    assert_eq!(admitted.field("access"), Some("roles"));
    assert_eq!(admitted.field("user"), Some(TEST_USER));
    assert_eq!(admitted.field("grpc.status"), Some("Ok"));

    let denied = &spans[2];
    assert_eq!(denied.field("access"), Some("roles"));
    assert_eq!(denied.field("user"), Some(TEST_USER));
    assert_eq!(denied.field("grpc.status"), Some("PermissionDenied"));

    let anonymous = &spans[3];
    assert_eq!(anonymous.field("access"), Some("roles"));
    assert!(!anonymous.has_field("user"));
    assert_eq!(anonymous.field("grpc.status"), Some("Unauthenticated"));
}

/// Hand-written table that omits `getEndPointTwo` (the macro cannot), to
/// show the gate fails closed.
struct Forgetful {
    entered: Arc<AtomicUsize>,
}

#[rust_grpc_lib::async_trait]
impl ExampleDb for Forgetful {
    async fn get_end_point_one(
        &self,
        _request: Request<IdList>,
    ) -> Result<Response<ReplyOne>, Status> {
        self.entered.fetch_add(1, Ordering::SeqCst);
        Ok(Response::new(reply_one("x")))
    }

    async fn get_end_point_two(
        &self,
        _request: Request<IdList>,
    ) -> Result<Response<ReplyTwo>, Status> {
        self.entered.fetch_add(1, Ordering::SeqCst);
        Ok(Response::new(reply_two("x")))
    }
}

impl ServicePolicy<ExampleDbServer<Forgetful>> for Forgetful {
    const POLICIES: &'static [MethodPolicy] =
        &[MethodPolicy::new("get_end_point_one", Access::Public)];
}

#[tokio::test]
async fn a_method_missing_from_the_table_is_denied_even_with_a_valid_token() {
    let (logs, _guard) = capture_logs();
    let entered = Arc::new(AtomicUsize::new(0));
    let running = serve(GrpcServer::builder(make_validator()).add_service(
        ExampleDbServer::new,
        Forgetful {
            entered: Arc::clone(&entered),
        },
    ))
    .await;

    let listed = client(running.addr, "").get_end_point_one(ids()).await;
    let unlisted = client(running.addr, &make_jwt(&["operator", "admin"]))
        .get_end_point_two(ids())
        .await
        .unwrap_err();

    assert!(listed.is_ok(), "the listed public method is reachable");
    assert_eq!(unlisted.code(), Code::PermissionDenied);
    assert_eq!(
        entered.load(Ordering::SeqCst),
        1,
        "only the listed method ran"
    );
    let text = logs.text();
    assert!(text.contains("ERROR"), "{text}");
    assert!(text.contains("no access policy"), "{text}");
}

#[tokio::test]
async fn the_table_driven_helper_never_enters_a_handler_of_a_generated_server() {
    let (running, calls) = start_gated().await;

    assert_policy_table_conforms(connect(running.addr).await, ExampleDbServer::<Gated>::new).await;

    // The helper sends Rust names (`get_end_point_two`): the gate matches
    // them, but tonic routes only proto names, so even the public method is
    // answered `Unimplemented`. See `rust_grpc_lib::testing`.
    assert_eq!(calls.count(), 0);
}

/// Requires a token on both methods, to disagree with `WrongTable` below.
struct Mislabeled;

#[access_controlled_service]
#[rust_grpc_lib::async_trait]
impl ExampleDb for Mislabeled {
    #[any_authenticated]
    async fn get_end_point_one(
        &self,
        ctx: RequestContext<IdList>,
    ) -> Result<Response<ReplyOne>, Status> {
        Ok(Response::new(reply_one(ctx.sub())))
    }

    #[any_authenticated]
    async fn get_end_point_two(
        &self,
        ctx: RequestContext<IdList>,
    ) -> Result<Response<ReplyTwo>, Status> {
        Ok(Response::new(reply_two(ctx.sub())))
    }
}

#[tokio::test]
#[should_panic(expected = "service conformance failed for 1 reason(s)")]
async fn the_table_driven_helper_reports_a_server_that_disagrees_with_the_table() {
    // The server enforces `Mislabeled`'s own table; this one claims
    // `getEndPointTwo` is public, which the server rejects.
    struct WrongTable;
    impl ServicePolicy<ExampleDbServer<Mislabeled>> for WrongTable {
        const POLICIES: &'static [MethodPolicy] = &[
            MethodPolicy::new("get_end_point_one", Access::Authenticated),
            MethodPolicy::new("get_end_point_two", Access::Public),
        ];
    }
    let running =
        serve(GrpcServer::builder(make_validator()).add_service(ExampleDbServer::new, Mislabeled))
            .await;

    // Never called: it only names `WrongTable` and the server it claims.
    let wrong_table = |_: WrongTable| -> ExampleDbServer<Mislabeled> {
        unreachable!("the constructor only names the types")
    };
    assert_policy_table_conforms(connect(running.addr).await, wrong_table).await;
}

#[tokio::test]
async fn the_conformance_helper_accepts_a_correctly_wired_service() {
    let (running, _calls) = start_gated().await;

    assert_service_conforms(
        connect(running.addr).await,
        ExampleDbServer::<Gated>::new,
        &["getEndPointOne", "getEndPointTwo"],
    )
    .await;
}

#[tokio::test]
#[should_panic(expected = "get_end_point_two: in the policy table but not in the list")]
async fn the_conformance_helper_flags_a_method_left_out_of_the_list() {
    let (running, _calls) = start_gated().await;

    assert_service_conforms(
        connect(running.addr).await,
        ExampleDbServer::<Gated>::new,
        &["getEndPointOne"],
    )
    .await;
}

#[tokio::test]
#[should_panic(expected = "deleteEverything: not in the policy table")]
async fn the_conformance_helper_flags_a_method_the_table_does_not_know() {
    let (running, _calls) = start_gated().await;

    assert_service_conforms(
        connect(running.addr).await,
        ExampleDbServer::<Gated>::new,
        &["getEndPointOne", "getEndPointTwo", "deleteEverything"],
    )
    .await;
}

#[tokio::test]
#[should_panic(expected = "no methods given")]
async fn the_conformance_helper_refuses_an_empty_list() {
    let (running, _calls) = start_gated().await;

    assert_service_conforms(
        connect(running.addr).await,
        ExampleDbServer::<Gated>::new,
        &[],
    )
    .await;
}

/// Echoes the caller's `sub`.
struct Downstream;

#[access_controlled_service]
#[rust_grpc_lib::async_trait]
impl ExampleDb for Downstream {
    #[any_authenticated]
    async fn get_end_point_one(
        &self,
        ctx: RequestContext<IdList>,
    ) -> Result<Response<ReplyOne>, Status> {
        Ok(Response::new(reply_one(ctx.sub())))
    }

    #[any_authenticated]
    async fn get_end_point_two(
        &self,
        ctx: RequestContext<IdList>,
    ) -> Result<Response<ReplyTwo>, Status> {
        Ok(Response::new(reply_two(ctx.sub())))
    }
}

/// Forwards the caller's token to [`Downstream`]: via `ctx.token()` in a
/// generated client (`getEndPointOne`), and via `ctx.forward(..)` on a plain
/// request (`getEndPointTwo`).
struct Upstream {
    downstream: String,
}

#[access_controlled_service]
#[rust_grpc_lib::async_trait]
impl ExampleDb for Upstream {
    #[any_authenticated]
    async fn get_end_point_one(
        &self,
        ctx: RequestContext<IdList>,
    ) -> Result<Response<ReplyOne>, Status> {
        let mut downstream = ExampleDbClient::authenticated(&self.downstream, ctx.token())
            .map_err(|e| Status::internal(e.to_string()))?;
        downstream.get_end_point_one(ctx.message).await
    }

    #[any_authenticated]
    async fn get_end_point_two(
        &self,
        ctx: RequestContext<IdList>,
    ) -> Result<Response<ReplyTwo>, Status> {
        let mut downstream = ExampleDbClient::public(&self.downstream)
            .map_err(|e| Status::internal(e.to_string()))?;
        downstream
            .get_end_point_two(ctx.forward(ctx.message.clone()))
            .await
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_handler_forwards_the_callers_identity_with_one_call() {
    let downstream =
        serve(GrpcServer::builder(make_validator()).add_service(ExampleDbServer::new, Downstream))
            .await;
    let upstream = serve(GrpcServer::builder(make_validator()).add_service(
        ExampleDbServer::new,
        Upstream {
            downstream: format!("http://{}", downstream.addr),
        },
    ))
    .await;
    let token = test_token(TEST_KID, "carol", &[]);

    let through_the_client = client(upstream.addr, &token)
        .get_end_point_one(ids())
        .await
        .expect("`ctx.token()` plugs into a generated client")
        .into_inner();
    let through_a_request = client(upstream.addr, &token)
        .get_end_point_two(ids())
        .await
        .expect("`ctx.forward(..)` sets the header on a plain request")
        .into_inner();

    assert_eq!(
        through_the_client.section_one[0]
            .part_one
            .as_ref()
            .unwrap()
            .some_string_alpha,
        "carol",
        "the downstream handler saw the outer caller's sub"
    );
    assert_eq!(
        through_a_request.section_two[0]
            .part_three
            .as_ref()
            .unwrap()
            .some_string_delta,
        "carol"
    );
}
