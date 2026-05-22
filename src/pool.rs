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
//! use rust_grpc_lib::register_client;
//! use rust_grpc_lib::types::services::alarm_commands::alarm_commands_client::AlarmCommandsClient;
//!
//! register_client!(AlarmCommandsClient);
//!
//! #[tokio::main]
//! async fn main() -> Result<(), tonic::transport::Error> {
//!     let client: AlarmCommandsClient<_> = rust_grpc_lib::pool::client("http://alarm-host:50051")?;
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

use crate::FromChannel;

type ChannelPool = RwLock<HashMap<String, Channel>>;

const KEEP_ALIVE_INTERVAL_VAR: &str = "RUST_GRPC_LIB_KEEP_ALIVE_INTERVAL_SECS";
const KEEP_ALIVE_INTERVAL_DEFAULT: u64 = 30;
const KEEP_ALIVE_TIMEOUT_VAR: &str = "RUST_GRPC_LIB_KEEP_ALIVE_TIMEOUT_SECS";
const KEEP_ALIVE_TIMEOUT_DEFAULT: u64 = 10;

static POOL: LazyLock<ChannelPool> = LazyLock::new(RwLock::default);

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
/// # Type parameter
///
/// `C` must implement [`FromChannel`]. Use [`register_client!`](crate::register_client)
/// to generate that implementation for any tonic-generated client type.
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
/// use rust_grpc_lib::register_client;
/// use rust_grpc_lib::types::services::alarm_commands::alarm_commands_client::AlarmCommandsClient;
///
/// register_client!(AlarmCommandsClient);
///
/// #[tokio::main]
/// async fn main() -> Result<(), tonic::transport::Error> {
///     let client: AlarmCommandsClient<_> = rust_grpc_lib::pool::client("http://alarm-host:50051")?;
///     Ok(())
/// }
/// ```
pub fn client<C: FromChannel>(endpoint: &str) -> Result<C, Error> {
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
fn get_or_create_channel(endpoint: &str, pool: &ChannelPool) -> Result<Channel, Error> {
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
        .http2_keep_alive_interval(read_duration_val(
            KEEP_ALIVE_INTERVAL_VAR,
            KEEP_ALIVE_INTERVAL_DEFAULT,
        ))
        .keep_alive_timeout(read_duration_val(
            KEEP_ALIVE_TIMEOUT_VAR,
            KEEP_ALIVE_TIMEOUT_DEFAULT,
        ))
        .keep_alive_while_idle(true)
        .connect_lazy();

    lock.insert(endpoint.to_string(), channel.clone());
    Ok(channel)
}

fn read_duration_val(var_name: &str, preset_default: u64) -> Duration {
    let seconds = env_var::get(var_name).or(preset_default);
    Duration::from_secs(seconds)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_pool() -> ChannelPool {
        RwLock::new(HashMap::new())
    }

    #[tokio::test]
    async fn valid_endpoint_returns_ok() {
        let pool = empty_pool();
        let result = get_or_create_channel("http://localhost:50051", &pool);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn invalid_endpoint_returns_err() {
        let pool = empty_pool();
        // An empty string is not a valid URI.
        let result = get_or_create_channel("", &pool);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn same_endpoint_reuses_channel() {
        let pool = empty_pool();
        let _ = get_or_create_channel("http://localhost:50051", &pool).unwrap();
        let _ = get_or_create_channel("http://localhost:50051", &pool).unwrap();
        // tonic::transport::Channel doesn't expose pointer equality directly,
        // but we can assert the pool only has one entry — proving no second
        // channel was created.
        assert_eq!(pool.read().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn different_endpoints_get_different_channels() {
        let pool = empty_pool();
        let _ = get_or_create_channel("http://localhost:50051", &pool).unwrap();
        let _ = get_or_create_channel("http://localhost:50052", &pool).unwrap();
        assert_eq!(pool.read().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn channel_is_inserted_into_pool() {
        let pool = empty_pool();
        assert!(pool.read().unwrap().is_empty());
        get_or_create_channel("http://localhost:50051", &pool).unwrap();
        assert!(pool.read().unwrap().contains_key("http://localhost:50051"));
    }
}
