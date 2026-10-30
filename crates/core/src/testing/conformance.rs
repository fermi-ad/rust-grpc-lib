//! Black-box authentication conformance checks that run against a live server.
//!
//! The library cannot enumerate the RPCs of a consumer's service, so the
//! caller passes an explicit list of method paths of the form
//! `"/package.Service/Method"`. Each check sends an empty message to every
//! listed path through a [`Channel`] and compares the status that comes back.
//! Rejections happen before a handler's business logic, so the empty message
//! is enough; the checks only ever expect an error.
//!
//! Health (and reflection) services are unauthenticated by construction, so
//! their paths are refused: listing one is a mistake in the test, not in the
//! server.
//!
//! The checks panic on the first failed call set, naming every path and the
//! status it actually returned, so they can be used directly in a test.

use std::str::FromStr;

use tonic::client::Grpc;
use tonic::codegen::http::uri::PathAndQuery;
use tonic::metadata::MetadataValue;
use tonic::transport::Channel;
use tonic::{Code, Request, Status};
use tonic_prost::ProstCodec;

#[cfg(test)]
mod tests;

/// Service-name prefixes that are open by design and must not be listed.
const OPEN_SERVICE_PREFIXES: [&str; 2] = ["/grpc.health.v1.Health/", "/grpc.reflection."];

/// A token that is not a JWT, used for the invalid-token check.
const INVALID_TOKEN: &str = "not-a-valid-jwt";

/// Assert that every RPC in `rpc_paths` rejects unauthenticated callers.
///
/// Two calls are made to each path, and both must return
/// [`Code::Unauthenticated`]:
///
/// 1. with no `Authorization` header, and
/// 2. with a bearer token that is not a valid JWT.
///
/// `rpc_paths` are `"/package.Service/Method"` strings. Do not include the
/// health service; the helper panics if you do.
///
/// ```rust,ignore
/// let channel = Channel::from_shared(format!("http://{addr}"))?.connect().await?;
/// assert_all_rpcs_require_auth(
///     channel,
///     &[
///         "/services.example.v1.ExampleDB/GetEndPointOne",
///         "/services.example.v1.ExampleDB/GetEndPointTwo",
///     ],
/// )
/// .await;
/// ```
///
/// # Panics
///
/// Panics if `rpc_paths` is empty or malformed, names an open service, or if
/// any call returns something other than `Unauthenticated`.
pub async fn assert_all_rpcs_require_auth(channel: Channel, rpc_paths: &[&str]) {
    check_paths(rpc_paths).unwrap_or_else(|reason| panic!("{reason}"));

    let cases = [
        ("no token", None, Code::Unauthenticated),
        ("invalid token", Some(INVALID_TOKEN), Code::Unauthenticated),
    ];
    run_cases(channel, rpc_paths, &cases).await;
}

/// Assert that every RPC in `rpc_paths` denies a valid token that lacks the
/// required role.
///
/// `token_without_role` must be a token the server's validator accepts but
/// whose claims do not satisfy the roles of the listed RPCs. Every call must
/// return [`Code::PermissionDenied`], so list only RPCs that declare roles
/// (for example with `#[roles(..)]`).
///
/// # Panics
///
/// Panics if `rpc_paths` is empty or malformed, names an open service, or if
/// any call returns something other than `PermissionDenied`.
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

/// Call one RPC with an empty message and discard any response.
///
/// `token` becomes an `Authorization: Bearer` header when `Some`. Returns the
/// status the server answered with, or `Ok(())` if the call succeeded. This is
/// the primitive behind the `assert_*` helpers, exposed for custom checks.
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

/// Run every `(label, token, expected code)` case against every path and panic
/// once, listing all mismatches.
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

/// Reject lists that would make a conformance check vacuous or wrong.
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

/// Whether `path` looks like `/<service>/<method>` with no whitespace.
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
