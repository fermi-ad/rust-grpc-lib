//! Process-wide gRPC connection pool.
//!
//! Channels are keyed by endpoint string and created lazily. Once a channel
//! exists for a given endpoint, all subsequent calls with that endpoint reuse
//! it. The pool is safe to use from multiple threads.
//!
//! # Public entry point
//!
//! [`get_channel`] is the only public function in this module. It returns a
//! [`tonic::transport::Channel`] for the given endpoint, creating and caching
//! one if none exists yet.
//!
//! Consumers do not typically call [`get_channel`] directly. Instead, the
//! `from_endpoint_with_provider` and `from_endpoint` constructors generated on
//! every client struct by `#[derive(GrpcClient)]` / `#[derive(GrpcNoAuthClient)]`
//! call it internally.
//!
//! # Tokio runtime requirement
//!
//! This module requires a [Tokio](https://tokio.rs) runtime. Tonic's transport
//! layer is built on Tokio and there is no way to use it without one.
//!
//! ```rust,ignore
//! // Typical usage — via the generated constructor, not get_channel directly:
//! use crate::proto::services::alarm_commands::alarm_commands_client::AlarmCommandsClient;
//! use rust_grpc_lib::auth::FileTokenProvider;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let provider = FileTokenProvider::from_env()?;
//!     let client = AlarmCommandsClient::from_endpoint_with_provider(
//!         "http://alarm-host:50051",
//!         provider,
//!     )?;
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

#[cfg(test)]
mod tests;

type ChannelMap = RwLock<HashMap<String, Channel>>;

const KEEP_ALIVE_INTERVAL_VAR: &str = "RUST_GRPC_LIB_KEEP_ALIVE_INTERVAL_SECS";
const KEEP_ALIVE_INTERVAL_DEFAULT: u64 = 30;
const KEEP_ALIVE_TIMEOUT_VAR: &str = "RUST_GRPC_LIB_KEEP_ALIVE_TIMEOUT_SECS";
const KEEP_ALIVE_TIMEOUT_DEFAULT: u64 = 10;

static POOL: LazyLock<ChannelMap> = LazyLock::new(RwLock::default);

pub fn get_channel(endpoint: &str) -> Result<Channel, Error> {
    get_or_create_channel(endpoint, &POOL)
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
