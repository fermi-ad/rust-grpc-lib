//! Black-box auth checks against a live server.
//!
//! Every check sends an empty message over a [`Channel`] and compares the
//! status; the gate rejects before any handler runs, so the message content
//! does not matter. Each `assert_*` makes all its calls, then panics once
//! listing every mismatch. Path-based checks take `"/package.Service/Method"`
//! (build it from the `SERVICE_NAME` const tonic generates in each `*_server`
//! module) and refuse health and reflection paths, which are open by design.

use std::str::FromStr;

use tonic::client::Grpc;
use tonic::codegen::http::uri::PathAndQuery;
use tonic::metadata::MetadataValue;
use tonic::server::NamedService;
use tonic::transport::Channel;
use tonic::{Code, Request, Status};
use tonic_prost::ProstCodec;

use crate::auth::policy::{Access, PolicyTable, ServicePolicy, same_method};

#[cfg(test)]
mod tests;

// Service-name prefixes that are open by design and must not be listed.
const OPEN_SERVICE_PREFIXES: [&str; 2] = ["/grpc.health.v1.Health/", "/grpc.reflection."];

// A token that is not a JWT, used for the invalid-token check.
const INVALID_TOKEN: &str = "not-a-valid-jwt";

/// Asserts that a served service enforces the policy table that
/// `#[access_controlled_service]` emitted, for every method in it. Start here.
///
/// `constructor` is the generated server's, as in `add_service`
/// (`FooServer::<Impl>::new`); it is never called, and names the server `S`
/// and the annotated implementation `T`. Methods come from
/// [`PolicyTable::for_service(constructor)`](PolicyTable::for_service), so
/// there is no list to maintain. Each is called at `/<S::NAME>/<method>`:
///
/// - `#[public]`: must not be answered `Unauthenticated` or
///   `PermissionDenied` with no token (anything else passes, including a
///   handler error).
/// - Otherwise: must answer `Unauthenticated` both with no token and with a
///   bearer token that is not a valid JWT.
///
/// Not checked:
///
/// - The success path and role denial (no valid token is sent); see
///   [`assert_all_rpcs_require_role`].
/// - Methods of the generated service absent from the table (the gate denies
///   them `PermissionDenied "method not permitted"`): nothing at run time
///   enumerates a tonic server's methods. The compiler normally closes this
///   gap (tonic traits require every method, the macro requires a policy on
///   each); for an independent check, such as a trait with default methods,
///   use [`assert_service_conforms`].
/// - That public handlers run. The table holds Rust names (`get_end_point_one`);
///   the gate matches them to proto names (`getEndPointOne`), so the access
///   decision is real, but tonic routes by exact proto name and answers
///   `Unimplemented`. [`assert_service_conforms`] sends proto names.
///
/// ```rust,no_run
/// # use std::marker::PhantomData;
/// # use rust_grpc_lib::{access_controlled_service, auth::RequestContext};
/// # use rust_grpc_lib::tonic::{Request, Response, Status, server::NamedService};
/// # trait ExampleDb {
/// #     async fn get(&self, r: Request<()>) -> Result<Response<()>, Status>;
/// # }
/// # struct ExampleDbServer<T>(PhantomData<T>);
/// # impl<T> ExampleDbServer<T> { fn new(_inner: T) -> Self { Self(PhantomData) } }
/// # impl<T> NamedService for ExampleDbServer<T> { const NAME: &'static str = "example.ExampleDB"; }
/// # struct MyDb;
/// # #[access_controlled_service]
/// # impl ExampleDb for MyDb {
/// #     #[any_authenticated]
/// #     async fn get(&self, ctx: RequestContext<()>) -> Result<Response<()>, Status> { todo!() }
/// # }
/// # async fn demo(channel: rust_grpc_lib::tonic::transport::Channel) {
/// use rust_grpc_lib::testing::assert_policy_table_conforms;
///
/// assert_policy_table_conforms(channel, ExampleDbServer::<MyDb>::new).await;
/// # }
/// ```
///
/// # Panics
///
/// Once, with `service conformance failed for N reason(s)` and one line per
/// reason, if the table is empty or any call is answered otherwise.
pub async fn assert_policy_table_conforms<T, S>(channel: Channel, constructor: impl FnOnce(T) -> S)
where
    S: NamedService,
    T: ServicePolicy<S>,
{
    let table = PolicyTable::for_service(constructor);
    let mut problems = Vec::new();
    if table.policies().next().is_none() {
        problems.push(
            "the policy table is empty; the check would pass without calling anything".into(),
        );
    }

    for policy in table.policies() {
        // The gate compares names ignoring case and underscores, so the Rust
        // spelling recorded in the table reaches the same policy as the proto
        // spelling does.
        let path = format!("/{}/{}", S::NAME, policy.method);
        if let Err(problem) = check_access(channel.clone(), &path, policy.access).await {
            problems.push(problem);
        }
    }

    fail_on(&problems);
}

/// Like [`assert_policy_table_conforms`] (same checks and failure format), but
/// for a hand-written list of proto method names, such as
/// `&["getEndPointOne", "getEndPointTwo"]`.
///
/// Use it when you want an independent statement of what the service
/// exposes: the list must match the table exactly in both directions, and
/// calls use the proto spelling, so `#[public]` handlers are reached.
/// `constructor` is the generated server's, never called, as for
/// [`assert_policy_table_conforms`].
///
/// ```rust,no_run
/// # use std::marker::PhantomData;
/// # use rust_grpc_lib::{access_controlled_service, auth::RequestContext};
/// # use rust_grpc_lib::tonic::{Request, Response, Status, server::NamedService};
/// # trait ExampleDb {
/// #     async fn get_end_point_one(&self, r: Request<()>) -> Result<Response<()>, Status>;
/// # }
/// # struct ExampleDbServer<T>(PhantomData<T>);
/// # impl<T> ExampleDbServer<T> { fn new(_inner: T) -> Self { Self(PhantomData) } }
/// # impl<T> NamedService for ExampleDbServer<T> { const NAME: &'static str = "example.ExampleDB"; }
/// # struct MyDb;
/// # #[access_controlled_service]
/// # impl ExampleDb for MyDb {
/// #     #[any_authenticated]
/// #     async fn get_end_point_one(&self, ctx: RequestContext<()>) -> Result<Response<()>, Status> { todo!() }
/// # }
/// # async fn demo(channel: rust_grpc_lib::tonic::transport::Channel) {
/// use rust_grpc_lib::testing::assert_service_conforms;
///
/// assert_service_conforms(channel, ExampleDbServer::<MyDb>::new, &["getEndPointOne"]).await;
/// # }
/// ```
///
/// # Panics
///
/// If `methods` is empty, names a method not in the table (the gate denies
/// it), omits one that is, or any call is answered otherwise.
pub async fn assert_service_conforms<T, S>(
    channel: Channel,
    constructor: impl FnOnce(T) -> S,
    methods: &[&str],
) where
    S: NamedService,
    T: ServicePolicy<S>,
{
    let table = PolicyTable::for_service(constructor);
    let mut problems = table_problems(&table, methods);

    for method in methods {
        let Some(access) = table.lookup(method) else {
            continue;
        };
        let path = format!("/{}/{method}", S::NAME);
        if let Err(problem) = check_access(channel.clone(), &path, access).await {
            problems.push(problem);
        }
    }

    fail_on(&problems);
}

// The failure format shared by the two table-based helpers.
fn fail_on(problems: &[String]) {
    assert!(
        problems.is_empty(),
        "service conformance failed for {} reason(s):\n  {}",
        problems.len(),
        problems.join("\n  ")
    );
}

/// Asserts that every path in `rpc_paths` answers [`Code::Unauthenticated`]
/// both with no `Authorization` header and with a bearer token that is not a
/// valid JWT. List only non-`#[public]` methods.
///
/// ```rust,no_run
/// # mod example_db_server { pub const SERVICE_NAME: &str = "example.ExampleDB"; }
/// # async fn demo(channel: rust_grpc_lib::tonic::transport::Channel) {
/// use example_db_server::SERVICE_NAME; // generated by tonic
/// use rust_grpc_lib::testing::assert_all_rpcs_require_auth;
///
/// let one = format!("/{SERVICE_NAME}/getEndPointOne");
/// assert_all_rpcs_require_auth(channel, &[&one]).await;
/// # }
/// ```
///
/// # Panics
///
/// If `rpc_paths` is empty or malformed or names a health or reflection
/// service (before any call), or any call returns another status.
pub async fn assert_all_rpcs_require_auth(channel: Channel, rpc_paths: &[&str]) {
    check_paths(rpc_paths).unwrap_or_else(|reason| panic!("{reason}"));

    let cases = [
        ("no token", None, Code::Unauthenticated),
        ("invalid token", Some(INVALID_TOKEN), Code::Unauthenticated),
    ];
    run_cases(channel, rpc_paths, &cases).await;
}

/// Asserts that every path in `rpc_paths` answers [`Code::PermissionDenied`]
/// to `token_without_role`: a token the server's validator accepts (such as
/// one from [`test_token`](super::test_token)) lacking the required roles.
/// List only `#[roles(..)]` methods.
///
/// # Panics
///
/// As [`assert_all_rpcs_require_auth`], but expecting `PermissionDenied`.
pub async fn assert_all_rpcs_require_role(
    channel: Channel,
    rpc_paths: &[&str],
    token_without_role: &str,
) {
    check_paths(rpc_paths).unwrap_or_else(|reason| panic!("{reason}"));

    let cases = [(
        "valid token without the required role",
        Some(token_without_role),
        Code::PermissionDenied,
    )];
    run_cases(channel, rpc_paths, &cases).await;
}

/// Calls `rpc_path` once with an empty message, sending `token` (if any) as
/// `Authorization: Bearer`, and discards the response. The primitive behind
/// the `assert_*` checks.
///
/// # Errors
///
/// - `InvalidArgument`, without sending, if `rpc_path` is not a valid URI
///   path or `token` is not a valid header value.
/// - A status converted from the transport error if the channel never becomes
///   ready.
/// - Otherwise, the server's status (such as `Unauthenticated`).
pub async fn call_rpc(channel: Channel, rpc_path: &str, token: Option<&str>) -> Result<(), Status> {
    let path = PathAndQuery::from_str(rpc_path)
        .map_err(|e| Status::invalid_argument(format!("bad RPC path {rpc_path:?}: {e}")))?;

    let mut request = Request::new(());
    if let Some(token) = token {
        let value = MetadataValue::try_from(format!("Bearer {token}"))
            .map_err(|e| Status::invalid_argument(format!("token is not a valid header: {e}")))?;
        request.metadata_mut().insert("authorization", value);
    }

    let mut grpc = Grpc::new(channel);
    grpc.ready()
        .await
        .map_err(|e| Status::from_error(e.into()))?;
    grpc.unary(request, path, ProstCodec::<(), ()>::default())
        .await
        .map(|_| ())
}

// Mismatches between `methods` and `table` in both directions. An empty list
// is an error so the check never passes vacuously.
fn table_problems(table: &PolicyTable, methods: &[&str]) -> Vec<String> {
    if methods.is_empty() {
        return vec!["no methods given; an empty list would pass without checking anything".into()];
    }
    let mut problems: Vec<String> = methods
        .iter()
        .filter(|method| table.lookup(method).is_none())
        .map(|method| {
            format!("{method}: not in the policy table, so the gate denies it; is it annotated?")
        })
        .collect();
    problems.extend(
        table
            .policies()
            .filter(|policy| {
                !methods
                    .iter()
                    .any(|method| same_method(method, policy.method))
            })
            .map(|policy| format!("{}: in the policy table but not in the list", policy.method)),
    );
    problems
}

// Calls `path` without valid credentials and compares with `access`. A public
// method need only avoid the gate's rejections.
async fn check_access(channel: Channel, path: &str, access: Access) -> Result<(), String> {
    match access {
        Access::Public => match call_rpc(channel, path, None).await {
            Err(status) if is_rejection(&status) => Err(format!(
                "{path} (public, no token): expected it to be reachable, got {:?}: {}",
                status.code(),
                status.message()
            )),
            _ => Ok(()),
        },
        Access::Authenticated | Access::Roles { .. } => {
            for (label, token) in [("no token", None), ("invalid token", Some(INVALID_TOKEN))] {
                match call_rpc(channel.clone(), path, token).await {
                    Err(status) if status.code() == Code::Unauthenticated => {}
                    Ok(()) => {
                        return Err(format!(
                            "{path} ({label}): expected Unauthenticated, the call succeeded"
                        ));
                    }
                    Err(status) => {
                        return Err(format!(
                            "{path} ({label}): expected Unauthenticated, got {:?}: {}",
                            status.code(),
                            status.message()
                        ));
                    }
                }
            }
            Ok(())
        }
    }
}

// Whether a status is the gate refusing the call.
fn is_rejection(status: &Status) -> bool {
    matches!(
        status.code(),
        Code::Unauthenticated | Code::PermissionDenied
    )
}

// Runs every `(label, token, expected)` case on every path; panics once.
async fn run_cases(channel: Channel, rpc_paths: &[&str], cases: &[(&str, Option<&str>, Code)]) {
    let mut failures = Vec::new();

    for path in rpc_paths {
        for (label, token, expected) in cases {
            let outcome = call_rpc(channel.clone(), path, *token).await;
            let actual = match &outcome {
                Ok(()) => "the call succeeded".to_string(),
                Err(status) if status.code() == *expected => continue,
                Err(status) => format!("{:?}: {}", status.code(), status.message()),
            };
            failures.push(format!(
                "{path} ({label}): expected {expected:?}, got {actual}"
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "auth conformance failed for {} call(s):\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
}

// Rejects lists that would make a check vacuous (empty) or misleading (open
// services answer without credentials by design).
fn check_paths(rpc_paths: &[&str]) -> Result<(), String> {
    if rpc_paths.is_empty() {
        return Err(
            "no RPC paths given; an empty list would pass without checking anything".into(),
        );
    }

    for path in rpc_paths {
        if let Some(prefix) = OPEN_SERVICE_PREFIXES
            .iter()
            .find(|prefix| path.starts_with(**prefix))
        {
            return Err(format!(
                "{path} belongs to an open service ({prefix}...), which is \
                 unauthenticated by design; remove it from the list"
            ));
        }
        if !is_rpc_path(path) {
            return Err(format!(
                "{path:?} is not of the form \"/package.Service/Method\""
            ));
        }
    }

    Ok(())
}

// Whether `path` looks like `/<service>/<method>` with no whitespace.
fn is_rpc_path(path: &str) -> bool {
    let Some(rest) = path.strip_prefix('/') else {
        return false;
    };
    match rest.split_once('/') {
        Some((service, method)) => {
            !service.is_empty()
                && !method.is_empty()
                && !method.contains('/')
                && !path.chars().any(char::is_whitespace)
        }
        None => false,
    }
}
