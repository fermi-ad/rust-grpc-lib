//! The access gate: the one place that enforces what a method requires.
//!
//! Crate-private. `GrpcServer::add_service` wraps each generated server in an
//! [`AuthGate`] with its [`PolicyTable`]. On each call the gate:
//!
//! 1. parses the method from the path and looks it up in the table; an
//!    unparsable path or a missing method is `PermissionDenied` (logged at
//!    `error`);
//! 2. records `access` on the [`RpcSpan`];
//! 3. for a non-public method, validates the bearer token (`Unauthenticated`
//!    on failure), records `user` and checks the roles (`PermissionDenied`
//!    and a `warn` event);
//! 4. removes the `authorization` header, whatever the outcome;
//! 5. rejects the call itself, or inserts [`RpcMethod`] and, if
//!    authenticated, `Caller`, and calls the inner service.

// Maintainer notes:
//
// - Fail closed: never add a default-allow path.
// - The decision is `AuthGate::admit`, a plain function, so it can be tested
//   without a server.
// - Denials reveal nothing about the caller's grants: generic messages, and
//   `log_denied` never logs the token, other claims or held roles.
// - The validated token travels only inside `Caller`, never in metadata.
// - `TokenValidator::validate` is an in-memory check, so it runs inline.

use std::future::{Ready, ready};
use std::net::SocketAddr;
use std::sync::Arc;
use std::task::{Context, Poll};

use futures_util::future::Either;
use rust_auth_lib::{Claims, TokenValidator, keycloak::KeycloakClaims};
use tonic::Status;
use tonic::body::Body;
use tonic::codegen::Service;
use tonic::codegen::http::header::AUTHORIZATION;
use tonic::codegen::http::{HeaderMap, Request, Response};
use tonic::server::NamedService;
use tonic::transport::server::TcpConnectInfo;

use super::caller::Caller;
use super::context::{RpcMethod, RpcSpan};
use super::policy::{Access, PolicyTable};

#[cfg(test)]
mod tests;

// Wraps a generated server and enforces its `PolicyTable`. All services on a
// server share one validator, hence the `Arc`.
pub(crate) struct AuthGate<S, V> {
    inner: S,
    table: PolicyTable,
    validator: Arc<V>,
}

impl<S, V: TokenValidator> AuthGate<S, V> {
    // Wrap `inner` so that every call is checked against `table`, with tokens
    // validated by `validator`.
    pub(crate) fn new(inner: S, table: PolicyTable, validator: Arc<V>) -> Self {
        Self {
            inner,
            table,
            validator,
        }
    }

    // Decide whether a call may proceed. `rpc` is `None` for an unparsable
    // path. Returns the `Caller` for an authenticated call, `None` for a
    // public one, or the rejection.
    pub(crate) fn admit(
        &self,
        rpc: Option<&RpcMethod>,
        headers: &HeaderMap,
        remote_addr: Option<SocketAddr>,
        span: &RpcSpan,
    ) -> Result<Option<Caller>, Status> {
        let Some(rpc) = rpc else {
            tracing::error!(
                target: "rust_grpc_lib::authz",
                "request path is not of the form /<service>/<method>; denying"
            );
            return Err(Status::permission_denied("method not permitted"));
        };
        let Some(access) = self.table.lookup(&rpc.method) else {
            let (service, method) = (rpc.service.as_str(), rpc.method.as_str());
            tracing::error!(
                target: "rust_grpc_lib::authz",
                service,
                method,
                "method has no access policy in the service's policy table; denying"
            );
            return Err(Status::permission_denied("method not permitted"));
        };
        span.record_access(access.name());

        if access == Access::Public {
            return Ok(None);
        }

        let token = bearer_token(headers)
            .ok_or_else(|| Status::unauthenticated("missing Authorization header"))?;
        let claims: KeycloakClaims = self
            .validator
            .validate(token)
            .map_err(|e| Status::unauthenticated(e.to_string()))?;
        span.record_user(claims.sub());

        if !access.permits(&claims) {
            log_denied(&claims, rpc, access, remote_addr);
            return Err(Status::permission_denied("required role not present"));
        }
        Ok(Some(Caller::new(claims, token)))
    }
}

impl<S, V, B> Service<Request<B>> for AuthGate<S, V>
where
    S: Service<Request<B>, Response = Response<Body>>,
    V: TokenValidator,
{
    type Response = Response<Body>;
    type Error = S::Error;
    // The inner service's future for an admitted call, or the ready rejection.
    type Future = Either<S::Future, Ready<Result<Response<Body>, S::Error>>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut request: Request<B>) -> Self::Future {
        // Taken out so the trace layer's copy is reused only if it matches
        // the path; only admitted calls get it back below.
        let known = request.extensions_mut().remove::<RpcMethod>();
        let rpc = RpcMethod::from_path_reusing(request.uri().path(), known);
        let span = request
            .extensions()
            .get::<RpcSpan>()
            .cloned()
            .unwrap_or_else(RpcSpan::none);
        let remote_addr = request
            .extensions()
            .get::<TcpConnectInfo>()
            .and_then(TcpConnectInfo::remote_addr);

        let outcome = self.admit(rpc.as_ref(), request.headers(), remote_addr, &span);
        // The token must not reach a handler through its metadata, whatever the
        // policy: a public method would otherwise see (and could print) it.
        request.headers_mut().remove(AUTHORIZATION);

        match outcome {
            Err(status) => Either::Right(ready(Ok(status.into_http()))),
            Ok(caller) => {
                if let Some(rpc) = rpc {
                    request.extensions_mut().insert(rpc);
                }
                if let Some(caller) = caller {
                    request.extensions_mut().insert(caller);
                }
                Either::Left(self.inner.call(request))
            }
        }
    }
}

// Manual impl: `#[derive(Clone)]` would needlessly require `V: Clone`.
impl<S: Clone, V> Clone for AuthGate<S, V> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            table: self.table,
            validator: Arc::clone(&self.validator),
        }
    }
}

// The wrapped service keeps its name, so a health reporter's `set_serving`
// and tonic's routing keep working.
impl<S: NamedService, V> NamedService for AuthGate<S, V> {
    const NAME: &'static str = S::NAME;
}

// The token of an `Authorization: Bearer <token>` header (case-sensitive).
fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
}

// The `warn` event for a role denial: required roles only, never held ones.
fn log_denied(
    claims: &KeycloakClaims,
    rpc: &RpcMethod,
    access: Access,
    remote_addr: Option<SocketAddr>,
) {
    let Access::Roles { mode, roles } = access else {
        return;
    };
    let remote = remote_addr.map(|addr| addr.to_string());
    let (service, method) = (rpc.service.as_str(), rpc.method.as_str());
    tracing::warn!(
        target: "rust_grpc_lib::authz",
        user = claims.sub(),
        rpc.service = service,
        rpc.method = method,
        required_roles = ?roles,
        role_mode = mode.as_str(),
        remote_addr = remote.as_deref(),
        "authorization denied"
    );
}
