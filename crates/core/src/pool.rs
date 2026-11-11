//! Process-wide pool of lazily connected gRPC channels.
//!
//! [`get_channel`] backs the generated `authenticated` and `public`
//! constructors; call it yourself only to give a plain tonic
//! client a pooled channel.
//!
//! - One thread-safe entry per **normalized** endpoint, never evicted.
//!   Endpoints that differ only by scheme or host case (`HTTP://Host:50051`
//!   and `http://host:50051`) or by a lone trailing `/` (`http://h:50051` and
//!   `http://h:50051/`) share an entry. A different port (default ports are
//!   not stripped), userinfo, other path or query is a different entry. Auth
//!   lives in each client's interceptor, so clients with different token
//!   providers share a channel.
//! - Channels are built from the endpoint as given, with no TLS settings: the
//!   infra team handles TLS, not this library.
//! - Keep-alive is read from the environment once, when an endpoint's channel
//!   is created: `RUST_GRPC_LIB_KEEP_ALIVE_INTERVAL_SECS` (default 30) and
//!   `RUST_GRPC_LIB_KEEP_ALIVE_TIMEOUT_SECS` (default 10). An unset or
//!   unparsable value uses the default and logs a `tracing` warning.
//!   Keep-alive while idle is always on.

use std::{
    collections::HashMap,
    error::Error as StdError,
    fmt::{Display, Formatter, Result as FmtResult},
    sync::{LazyLock, PoisonError, RwLock},
    time::Duration,
};

use rust_env_var_lib::env_var;
use tokio::runtime::{Handle as TokioRuntimeHandle, TryCurrentError};
use tonic::transport::{Channel, Endpoint, Error};

#[cfg(test)]
mod tests;

/// Returns the pooled [`Channel`] for `endpoint`, creating it on first use.
///
/// Endpoints that differ only by scheme or host case, or by a lone trailing
/// `/`, share one channel (see the [module docs](self)). The first string
/// seen for an endpoint is the one the channel connects to.
///
/// Concurrent first calls for one endpoint create exactly one channel. It
/// connects lazily, so an unreachable host surfaces at the first RPC, not
/// here.
///
/// # Errors
///
/// - [`ChannelError::Runtime`]: called outside a Tokio runtime (checked on
///   every call, even for an already-pooled endpoint).
/// - [`ChannelError::Transport`]: `endpoint` is not a valid URI.
///
/// # Examples
///
/// ```no_run
/// # async fn demo() -> Result<(), rust_grpc_lib::pool::ChannelError> {
/// let channel = rust_grpc_lib::pool::get_channel("http://localhost:50051")?;
/// // Pass `channel` to any tonic client's `new`.
/// # let _ = channel;
/// # Ok(())
/// # }
/// ```
pub fn get_channel(endpoint: &str) -> Result<Channel, ChannelError> {
    get_or_create_channel(endpoint, &POOL)
}

/// Error of [`get_channel`] and of the generated `authenticated` and `public`
/// constructors.
///
/// `#[non_exhaustive]`: match with a wildcard arm. `Display` forwards the
/// wrapped error's message, which [`source`](StdError::source) also returns.
#[derive(Debug)]
#[non_exhaustive]
pub enum ChannelError {
    /// No Tokio runtime is running. Build clients inside `#[tokio::main]` or
    /// `#[tokio::test]`.
    Runtime(TryCurrentError),
    /// The endpoint is not a valid URI. Unreachable hosts are not reported
    /// here; they fail at the first RPC.
    Transport(Error),
}
impl Display for ChannelError {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        match self {
            Self::Runtime(inner) => write!(f, "{inner}"),
            Self::Transport(inner) => write!(f, "{inner}"),
        }
    }
}
impl From<Error> for ChannelError {
    fn from(value: Error) -> Self {
        Self::Transport(value)
    }
}
impl From<TryCurrentError> for ChannelError {
    fn from(value: TryCurrentError) -> Self {
        Self::Runtime(value)
    }
}
impl StdError for ChannelError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        let inner: &(dyn StdError + 'static) = match self {
            Self::Runtime(inner) => inner,
            Self::Transport(inner) => inner,
        };
        Some(inner)
    }
}

type ChannelMap = RwLock<HashMap<String, Channel>>;

const KEEP_ALIVE_INTERVAL_VAR: &str = "RUST_GRPC_LIB_KEEP_ALIVE_INTERVAL_SECS";
const KEEP_ALIVE_INTERVAL_DEFAULT: u64 = 30;
const KEEP_ALIVE_TIMEOUT_VAR: &str = "RUST_GRPC_LIB_KEEP_ALIVE_TIMEOUT_SECS";
const KEEP_ALIVE_TIMEOUT_DEFAULT: u64 = 10;

static POOL: LazyLock<ChannelMap> = LazyLock::new(RwLock::default);

// Takes the pool as a parameter so tests can use a private map.
//
// The endpoint is parsed first (an invalid URI fails here, pooled or not), and
// the pool key is derived from the parsed URI. The channel is built from that
// same `Endpoint`, so the key never changes what is connected to.
//
// Double-checked locking: the read lock serves the common pooled case; the
// lookup is repeated under the write lock, which is held while the channel is
// built and inserted, so one endpoint never gets two channels.
//
// Poison is ignored: the only mutation is a single `insert`, so the map is
// always valid.
fn get_or_create_channel(endpoint: &str, pool: &ChannelMap) -> Result<Channel, ChannelError> {
    let _ = TokioRuntimeHandle::try_current()?;
    let parsed = Endpoint::new(endpoint.to_string())?;
    let key = pool_key(&parsed, endpoint);
    if let Some(channel) = pool
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&key)
        .cloned()
    {
        return Ok(channel);
    }

    let mut lock = pool.write().unwrap_or_else(PoisonError::into_inner);
    if let Some(channel) = lock.get(&key).cloned() {
        return Ok(channel);
    }

    let channel = parsed
        .http2_keep_alive_interval(duration_from_env(
            KEEP_ALIVE_INTERVAL_VAR,
            KEEP_ALIVE_INTERVAL_DEFAULT,
        ))
        .keep_alive_timeout(duration_from_env(
            KEEP_ALIVE_TIMEOUT_VAR,
            KEEP_ALIVE_TIMEOUT_DEFAULT,
        ))
        .keep_alive_while_idle(true)
        .connect_lazy();

    lock.insert(key, channel.clone());
    Ok(channel)
}

// The pool key: `scheme://authority[path[?query]]`, with the scheme and host
// lowercased and a lone `/` path (no query) dropped. Userinfo, port, any other
// path and the query are kept as written, and default ports are not stripped,
// so entries that could differ stay distinct. A URI without a scheme or an
// authority has no safe normal form and is keyed by the raw string.
fn pool_key(endpoint: &Endpoint, raw: &str) -> String {
    let uri = endpoint.uri();
    let (Some(scheme), Some(authority)) = (uri.scheme_str(), uri.authority()) else {
        return raw.to_string();
    };
    // Only the host is case-insensitive; userinfo is not.
    let authority = match authority.as_str().rsplit_once('@') {
        Some((userinfo, host)) => format!("{userinfo}@{}", host.to_ascii_lowercase()),
        None => authority.as_str().to_ascii_lowercase(),
    };
    let mut key = format!("{}://{authority}", scheme.to_ascii_lowercase());
    match (uri.path(), uri.query()) {
        ("/", None) => {}
        (path, None) => key.push_str(path),
        (path, Some(query)) => {
            key.push_str(path);
            key.push('?');
            key.push_str(query);
        }
    }
    key
}

// `env_var::get(..).or` logs a warning whenever it falls back to the default.
fn duration_from_env(var_name: &str, default_secs: u64) -> Duration {
    let seconds = env_var::get(var_name).or(default_secs);
    Duration::from_secs(seconds)
}
