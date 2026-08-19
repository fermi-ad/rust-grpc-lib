//! Process-wide gRPC connection pool.
//!
//! Channels are keyed by endpoint string and created lazily. Once a channel
//! exists for a given endpoint, all subsequent calls with that endpoint reuse
//! it. The pool is safe to use from multiple threads.
//!
//! # Tokio runtime requirement
//!
//! This module requires a [Tokio](https://tokio.rs) runtime. Tonic's transport
//! layer is built on Tokio and there is no way to use it without one.
//!
//! ```rust,ignore
//! use rust_grpc_lib::proto::services::alarm_commands::alarm_commands_client::AlarmCommandsClient;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), tonic::transport::Error> {
//!     let client: AlarmCommandsClient<_> = rust_grpc_lib::pool::get("http://alarm-host:50051")?;
//!     Ok(())
//! }
//! ```

use std::{
    collections::HashMap,
    sync::{LazyLock, RwLock},
    time::Duration,
};

use rust_env_var_lib::env_var;
use tonic::transport::{Channel, Endpoint, Error};

use crate::GrpcClient;

#[cfg(feature = "auth")]
use crate::auth::TokenProvider;
#[cfg(feature = "auth")]
use crate::auth::interceptor::ClientJwtInterceptor;

#[cfg(test)]
mod tests;

type ChannelMap = RwLock<HashMap<String, Channel>>;

const KEEP_ALIVE_INTERVAL_VAR: &str = "RUST_GRPC_LIB_KEEP_ALIVE_INTERVAL_SECS";
const KEEP_ALIVE_INTERVAL_DEFAULT: u64 = 30;
const KEEP_ALIVE_TIMEOUT_VAR: &str = "RUST_GRPC_LIB_KEEP_ALIVE_TIMEOUT_SECS";
const KEEP_ALIVE_TIMEOUT_DEFAULT: u64 = 10;

static POOL: LazyLock<ChannelMap> = LazyLock::new(RwLock::default);

/// Return a gRPC client connected to `endpoint`, reusing an existing channel
/// if one has already been created for that endpoint.
///
/// The channel is connected lazily: no network activity occurs until the first
/// RPC is made on the returned client.
///
/// This function requires that you are already inside a Tokio runtime (e.g. inside
/// `#[tokio::main]` or an async service). If you are outside a Tokio runtime, the
/// call will panic.
///
/// # Type parameters
///
/// - `C` must implement [`GrpcClient`]. All generated tonic service clients in
///   this library implement [`GrpcClient`] automatically.
/// - `P` must implement [`TokenProvider`]. The provider is called on every
///   outbound request to attach a `Bearer` token via
///   [`crate::auth::interceptor::ClientJwtInterceptor`].
///
/// # Errors
///
/// Returns [`tonic::transport::Error`] if `endpoint` is not a valid URI.
///
/// # Panics
///
/// Panics if called outside of a Tokio runtime context.
///
/// # Example
///
/// ```rust,ignore
/// use rust_grpc_lib::proto::services::alarm_commands::alarm_commands_client::AlarmCommandsClient;
/// use rust_grpc_lib::auth::FileTokenProvider;
///
/// #[tokio::main]
/// async fn main() -> Result<(), tonic::transport::Error> {
///     let provider = FileTokenProvider::from_env()?;
///     let client: AlarmCommandsClient<_> =
///         rust_grpc_lib::pool::get("http://alarm-host:50051", provider)?;
///     Ok(())
/// }
/// ```
#[cfg(feature = "auth")]
pub fn get<C: GrpcClient, P: TokenProvider>(endpoint: &str, provider: P) -> Result<C, Error> {
    let channel = get_or_create_channel(endpoint, &POOL)?;
    Ok(C::from_channel_with_interceptor(
        channel,
        ClientJwtInterceptor { provider },
    ))
}

/// Return an **unauthenticated** gRPC client connected to `endpoint`, bypassing
/// JWT auth entirely.
///
/// Only available when the `unauthenticated` feature is enabled. Intended for local
/// development, integration tests, or environments where zero-trust auth is not
/// required. **Do not use in production.**
///
/// The channel is connected lazily: no network activity occurs until the first
/// RPC is made on the returned client.
///
/// This function requires that you are already inside a Tokio runtime (e.g. inside
/// `#[tokio::main]` or an async service). If you are outside a Tokio runtime, the
/// call will panic.
///
/// # Type parameter
///
/// `C` must implement [`GrpcClient`]. All generated tonic service clients in
/// this library implement [`GrpcClient`] automatically.
///
/// # Errors
///
/// Returns [`tonic::transport::Error`] if `endpoint` is not a valid URI.
///
/// # Panics
///
/// Panics if called outside of a Tokio runtime context.
///
/// # Example
///
/// ```rust,ignore
/// use rust_grpc_lib::proto::services::alarm_commands::alarm_commands_client::AlarmCommandsClient;
///
/// #[tokio::main]
/// async fn main() -> Result<(), tonic::transport::Error> {
///     // Only available with the `unauthenticated` feature enabled.
///     let client: AlarmCommandsClient<_> =
///         rust_grpc_lib::pool::get_unauthenticated("http://alarm-host:50051")?;
///     Ok(())
/// }
/// ```
#[cfg(feature = "unauthenticated")]
pub fn get_unauthenticated<C: GrpcClient>(endpoint: &str) -> Result<C, Error> {
    let channel = get_or_create_channel(endpoint, &POOL)?;
    Ok(C::from_channel(channel))
}

/// Look up an existing channel for `endpoint` in the pool, or create and
/// insert one if none exists.
///
/// Uses a double-checked lock: a read lock is taken first to avoid write
/// contention on the common path where the channel already exists.
///
/// ### Poison recovery
/// If the thread should panic (somehow) while the lock is held, the pool will
/// become "poisoned". The next read or write lock will return an error wrapping
/// the lock handle.
///
/// As the only mutations to the pool are insertions, the state of the pool should
/// always be valid. It is therefore safe to simply extract the lock handle from the
/// error and proceed.
fn get_or_create_channel(endpoint: &str, pool: &ChannelMap) -> Result<Channel, Error> {
    if let Some(channel) = pool
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .get(endpoint)
        .cloned()
    {
        return Ok(channel);
    }

    let mut lock = pool.write().unwrap_or_else(|e| e.into_inner());
    // Re-check after acquiring the write lock: another thread may have
    // inserted the channel between our read and write lock acquisitions.
    if let Some(channel) = lock.get(endpoint).cloned() {
        return Ok(channel);
    }

    let channel = Endpoint::new(endpoint.to_string())?
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

    lock.insert(endpoint.to_string(), channel.clone());
    Ok(channel)
}

fn duration_from_env(var_name: &str, default_secs: u64) -> Duration {
    let seconds = env_var::get(var_name).or(default_secs);
    Duration::from_secs(seconds)
}
