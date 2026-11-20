//! Tests for the conformance helpers.
//!
//! The path validation is tested directly. [`assert_policy_table_conforms`]
//! runs here against a hand-written stand-in for a generated server, behind the
//! real gate, so the table-driven checks (including the ones that must fail)
//! are covered without a generated service. The helpers against generated
//! services are covered in the `integration_tests` crate.

use super::*;

use std::convert::Infallible;
use std::future::{Ready, ready};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tonic::body::Body;
use tonic::codegen::Service;
use tonic::codegen::http::{Request as HttpRequest, Response as HttpResponse};

use crate::auth::policy::MethodPolicy;
use crate::server::GrpcServer;
use crate::testing::test_validator;

#[test]
fn well_formed_paths_are_accepted() {
    check_paths(&[
        "/services.example.v1.ExampleDB/GetEndPointOne",
        "/pkg.Svc/Method",
    ])
    .expect("valid paths");
}

#[test]
fn empty_list_is_rejected() {
    let err = check_paths(&[]).unwrap_err();

    assert!(err.contains("no RPC paths"), "{err}");
}

#[test]
fn open_service_paths_are_rejected() {
    for path in [
        "/grpc.health.v1.Health/Check",
        "/grpc.reflection.v1.ServerReflection/ServerReflectionInfo",
    ] {
        let err = check_paths(&["/pkg.Svc/Method", path]).unwrap_err();

        assert!(err.contains("open service"), "{path}: {err}");
    }
}

#[test]
fn malformed_paths_are_rejected() {
    for path in [
        "",
        "/",
        "pkg.Svc/Method",
        "/pkg.Svc",
        "/pkg.Svc/",
        "//Method",
        "/pkg.Svc/Method/Extra",
        "/pkg.Svc/Met hod",
    ] {
        let err = check_paths(&[path]).unwrap_err();

        assert!(err.contains("not of the form"), "{path:?}: {err}");
    }
}

// ---------------------------------------------------------------------------
// `assert_policy_table_conforms`
// ---------------------------------------------------------------------------

/// A stand-in for a generated server: it answers every call with the status
/// its `behavior` picks for the request path, and records the paths it saw.
#[derive(Clone)]
struct Stand {
    behavior: Arc<dyn Fn(&str) -> Status + Send + Sync>,
    seen: Arc<Mutex<Vec<String>>>,
}

impl tonic::server::NamedService for Stand {
    const NAME: &'static str = "pkg.Stand";
}

impl Service<HttpRequest<Body>> for Stand {
    type Response = HttpResponse<Body>;
    type Error = Infallible;
    type Future = Ready<Result<HttpResponse<Body>, Infallible>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: HttpRequest<Body>) -> Self::Future {
        let path = request.uri().path().to_owned();
        let status = (self.behavior)(&path);
        self.seen.lock().unwrap().push(path);
        ready(Ok(status.into_http()))
    }
}

impl Stand {
    fn new(behavior: impl Fn(&str) -> Status + Send + Sync + 'static) -> Self {
        Self {
            behavior: Arc::new(behavior),
            seen: Arc::default(),
        }
    }
}

/// The policies of the stand-in: one public, one authenticated and two
/// role-gated methods, written as Rust names like the macro does.
struct Table;

impl ServicePolicy<Stand> for Table {
    const POLICIES: &'static [MethodPolicy] = &[
        MethodPolicy::new("get_data", Access::any_of(&["operator"])),
        MethodPolicy::new("list_items", Access::Authenticated),
        MethodPolicy::new("ping", Access::Public),
        MethodPolicy::new("delete_item", Access::all_of(&["admin", "operator"])),
    ];
}

/// A table with no methods at all.
struct EmptyTable;

impl ServicePolicy<Stand> for EmptyTable {
    const POLICIES: &'static [MethodPolicy] = &[];
}

// Constructors for `for_service`-style arguments; they only name the types
// and are never called.
fn table_stand(_: Table) -> Stand {
    unreachable!("the constructor only names the types")
}

fn empty_stand(_: EmptyTable) -> Stand {
    unreachable!("the constructor only names the types")
}

/// A server that stops when dropped.
struct Live {
    addr: SocketAddr,
    seen: Arc<Mutex<Vec<String>>>,
    task: JoinHandle<()>,
}

impl Drop for Live {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Live {
    async fn channel(&self) -> Channel {
        Channel::from_shared(format!("http://{}", self.addr))
            .unwrap()
            .connect()
            .await
            .unwrap()
    }

    fn seen(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }
}

/// Serves the stand-in directly, with no gate, so its `behavior` alone decides
/// what a caller gets. That is how a service that disagrees with its table is
/// made.
async fn serve_ungated(behavior: impl Fn(&str) -> Status + Send + Sync + 'static) -> Live {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let stand = Stand::new(behavior);
    let seen = Arc::clone(&stand.seen);
    let task = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(stand)
            .serve_with_incoming(tonic::transport::server::TcpIncoming::from(listener))
            .await
            .unwrap();
    });
    Live { addr, seen, task }
}

/// Serves the stand-in behind the real gate, enforcing `Table`.
async fn serve_gated() -> Live {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let stand = Stand::new(|_| Status::unimplemented("the handler ran"));
    let seen = Arc::clone(&stand.seen);
    let builder = GrpcServer::builder(test_validator("conformance-kid"))
        .add_service(move |_: Table| stand, Table);
    let task = tokio::spawn(async move {
        builder
            .serve_on_listener(listener, std::future::pending())
            .await
            .unwrap();
    });
    Live { addr, seen, task }
}

/// The behavior of a service that matches `Table`: `ping` is open and
/// everything else is `Unauthenticated`.
fn conforming(path: &str) -> Status {
    if path.ends_with("/ping") {
        Status::unimplemented("the handler ran")
    } else {
        Status::unauthenticated("no token")
    }
}

/// Runs `check` on its own task and returns the message it panicked with.
async fn panic_message(check: impl Future<Output = ()> + Send + 'static) -> String {
    let error = tokio::spawn(check).await.expect_err("the check must panic");
    let payload = error.into_panic();
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_owned()))
        .expect("the panic carries a string")
}

#[tokio::test]
async fn a_gated_service_with_public_authenticated_and_role_gated_methods_conforms() {
    let live = serve_gated().await;

    assert_policy_table_conforms(live.channel().await, table_stand).await;

    // Only the public method gets past the gate to the handler.
    assert_eq!(live.seen(), ["/pkg.Stand/ping"]);
}

#[tokio::test]
async fn every_method_in_the_table_is_called_though_the_caller_gives_no_list() {
    let live = serve_ungated(conforming).await;

    assert_policy_table_conforms(live.channel().await, table_stand).await;

    let seen = live.seen();
    // `S::NAME` plus the method as the table spells it, for every entry: the
    // caller named none of them.
    for policy in PolicyTable::for_service(table_stand).policies() {
        let path = format!("/pkg.Stand/{}", policy.method);
        let calls = seen.iter().filter(|seen| **seen == path).count();
        let expected = if policy.access == Access::Public {
            1
        } else {
            2
        };
        assert_eq!(calls, expected, "{path}: {seen:?}");
    }
    assert_eq!(seen.len(), 7, "one call for ping, two for each of the rest");
}

#[tokio::test]
async fn a_public_method_that_is_rejected_though_the_table_opens_it_is_reported() {
    let live = serve_ungated(|_| Status::unauthenticated("no token")).await;
    let channel = live.channel().await;

    let message = panic_message(assert_policy_table_conforms(channel, table_stand)).await;

    assert!(
        message.contains("/pkg.Stand/ping (public, no token): expected it to be reachable"),
        "{message}"
    );
    assert!(
        message.starts_with("service conformance failed for 1 reason(s):"),
        "{message}"
    );
}

#[tokio::test]
async fn all_disagreements_are_reported_in_a_single_panic() {
    // Open to everyone: every method that needs a token disagrees with the table.
    let live = serve_ungated(|_| Status::unimplemented("the handler ran")).await;
    let channel = live.channel().await;

    let message = panic_message(assert_policy_table_conforms(channel, table_stand)).await;

    assert!(
        message.starts_with("service conformance failed for 3 reason(s):"),
        "{message}"
    );
    for method in ["get_data", "list_items", "delete_item"] {
        assert!(
            message.contains(&format!("/pkg.Stand/{method} (no token)")),
            "{message}"
        );
    }
    assert!(!message.contains("/pkg.Stand/ping"), "{message}");
}

#[tokio::test]
async fn an_empty_table_fails_instead_of_passing_without_checking_anything() {
    let live = serve_ungated(conforming).await;
    let channel = live.channel().await;

    let message = panic_message(assert_policy_table_conforms(channel, empty_stand)).await;

    assert!(
        message.starts_with("service conformance failed for 1 reason(s):"),
        "{message}"
    );
    assert!(message.contains("policy table is empty"), "{message}");
    assert_eq!(live.seen(), [] as [String; 0]);
}
