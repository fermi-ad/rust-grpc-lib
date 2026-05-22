//! gRPC client types and connection pool for Controls group services.
//!
//! This library provides:
//!
//! - **Generated message types** — protobuf structs and enums for all Controls
//!   gRPC services, available under [`proto`].
//! - **Generated service clients** — tonic client structs nested inside each
//!   service module under [`proto::services`].
//! - **Connection pooling** — a process-wide pool of lazily-connected channels
//!   via [`pool::get`], so all callers share connections without coordinating
//!   themselves.
//!
//! # Runtime requirement
//!
//! This library requires a [Tokio](https://tokio.rs) runtime. Tonic's transport
//! layer is built on Tokio; there is no way to use it without one.
//!
//! # Quick start
//!
//! 1. Register the client type you want to use with [`register_client!`].
//! 2. Call [`pool::get`] to obtain a client backed by a pooled channel.
//!
//! ```rust,ignore
//! use rust_grpc_lib::register_client;
//! use rust_grpc_lib::proto::services::alarm_commands::alarm_commands_client::AlarmCommandsClient;
//!
//! register_client!(AlarmCommandsClient);
//!
//! // Inside #[tokio::main] or any async context:
//! let client: AlarmCommandsClient<_> = rust_grpc_lib::pool::get("http://alarm-commands-host:50051")?;
//! ```
//!
//! # Connection pooling
//!
//! Channels are keyed by the endpoint string. Calling [`pool::get`] multiple
//! times with the same endpoint returns clients that share the same underlying
//! channel. Connections are established lazily on the first RPC, not at the
//! time [`pool::get`] is called.
//!
//! # Adding derive macros to generated types
//!
//! If you need generated structs or enums to implement additional traits (e.g.
//! `serde::Serialize`), set the following environment variables before building:
//!
//! | Variable | Applies to |
//! |---|---|
//! | `RUST_GRPC_LIB_ENUM_ATTRIBUTES` | enums |
//! | `RUST_GRPC_LIB_FIELD_ATTRIBUTES` | fields within structs |
//! | `RUST_GRPC_LIB_TYPE_ATTRIBUTES` | message structs |
//!
//! The format is semicolon-separated `proto.path=attribute` pairs:
//!
//! ```text
//! RUST_GRPC_LIB_TYPE_ATTRIBUTES="services.alarm_commands.AcknowledgeRequest=#[derive(serde::Serialize, serde::Deserialize)]"
//! ```
//!
//! A malformed pair (missing `=`) will stop the build with an error.
//!
//! # Keepalive configuration
//!
//! HTTP/2 keepalive probes are sent on every channel. The timing can be
//! overridden at runtime via environment variables:
//!
//! | Variable | Default | Meaning |
//! |---|---|---|
//! | `RUST_GRPC_LIB_KEEP_ALIVE_INTERVAL_SECS` | `30` | Seconds between keepalive pings |
//! | `RUST_GRPC_LIB_KEEP_ALIVE_TIMEOUT_SECS` | `10` | Seconds to wait for a keepalive ping acknowledgement before closing the connection |
//!
//! Keepalive pings are also sent while the connection is idle.

use tonic::transport::Channel;

pub mod pool;
pub mod proto;

/// Marker trait for gRPC client types that can be constructed from a [`Channel`].
///
/// Implement this trait via the [`register_client!`] macro rather than by hand.
#[diagnostic::on_unimplemented(
    message = "`{Self}` has not been registered as a gRPC client",
    label = "call `register_client!({Self})` before using this type with `pool::get`",
    note = "See the rust-grpc-lib README for usage examples"
)]
pub trait GrpcClient: Sized {
    /// Construct a client from an existing [`Channel`].
    fn from_channel(channel: Channel) -> Self;
}

/// Implement [`GrpcClient`] for a generated tonic client type.
///
/// Pass the bare client struct name (without the `<Channel>` type parameter).
/// The macro expands to an `impl GrpcClient for YourClient<tonic::transport::Channel>`,
/// which allows the type to be used with [`pool::get`].
///
/// # Example
///
/// ```rust,ignore
/// use rust_grpc_lib::register_client;
/// use rust_grpc_lib::proto::services::alarm_commands::alarm_commands_client::AlarmCommandsClient;
///
/// // Call this once at the top of the crate that uses the client.
/// register_client!(AlarmCommandsClient);
/// ```
#[macro_export]
macro_rules! register_client {
    ($client:ident) => {
        impl $crate::GrpcClient for $client<::tonic::transport::Channel> {
            fn from_channel(ch: ::tonic::transport::Channel) -> Self {
                <$client<::tonic::transport::Channel>>::new(ch)
            }
        }
    };
}
