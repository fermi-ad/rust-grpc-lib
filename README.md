# rust-grpc-lib

A Rust library that provides auto-generated gRPC client structs and handles connection pooling for the Controls group's gRPC services.

## What it does

- Compiles the `.proto` definitions from the [`interface-definitions`](https://github.com/fermi-ad/interface-definitions) repository at build time using [`tonic`](https://github.com/hyperium/tonic)
- Exposes the generated message types and service clients under `rust_grpc_lib::proto`
- Manages a process-wide pool of lazily-connected `Channel`s so callers share connections without coordinating themselves

## Adding as a dependency

This library is still internal to the Fermi-AD Github org. While we wait for approval to make it public, **only other internal projects will be able to use it**. Add it to your `Cargo.toml` like so:

```toml
[dependencies]
rust-grpc-lib = { git = "https://github.com/fermi-ad/rust-grpc-lib", tag = "vX.Y.Z" }
```

## Runtime requirement

This library requires a [Tokio](https://tokio.rs) runtime. Tonic's transport layer is built on Tokio and there is no way to use it without one.

## Getting a client

```rust
use rust_grpc_lib::pool;
use rust_grpc_lib::proto::services::alarm_commands::alarm_commands_client::AlarmCommandsClient;
use rust_grpc_lib::register_client;

register_client!(AlarmCommandsClient);

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let client: AlarmCommandsClient<_> = pool::get("http://alarm-commands-host:50051")?;
    // use client...
    Ok(())
}
```

`pool::get` returns a client backed by a shared, lazily-connected channel. Calling the function multiple times with the same endpoint string reuses the same underlying connection.

## Available services

| Module path | Service |
|---|---|
| `proto::services::alarm_commands` | Alarm Commands |
| `proto::services::alarm_groups` | Alarm Groups (DB) |
| `proto::services::alarm_timers` | Alarm Timers (DB) |
| `proto::services::alarm_user_layouts` | Alarm User Layouts (DB) |
| `proto::services::clock_event` | Clock Event (ACLK) |
| `proto::services::daq` | Data Acquisition (DAQ) |
| `proto::services::devdb` | Device Database (DevDB) |
| `proto::services::ioc_alarms` | IOC Alarms |
| `proto::services::tlg_placement` | TLG Placement |

Common message types shared across services are under `proto::common`.

## Adding `derive` macros to generated types (optional)

If you need the generated structs to implement additional traits (e.g. `serde::Serialize`), set the relevant environment variables before building:

| Variable | Applies to |
| -------- | ---------- |
| `RUST_GRPC_LIB_ENUM_ATTRIBUTES` | enums |
| `RUST_GRPC_LIB_FIELD_ATTRIBUTES` | fields within structs |
| `RUST_GRPC_LIB_TYPE_ATTRIBUTES` | message structs |

**Format**: semicolon-separated pairs of `proto-path=attribute`:
```
RUST_GRPC_LIB_TYPE_ATTRIBUTES="services.alarm_commands.AcknowledgeRequest=#[derive(serde::Serialize, serde::Deserialize)]"
```

Multiple attributes:
```
RUST_GRPC_LIB_TYPE_ATTRIBUTES="services.alarm_commands.AcknowledgeRequest=#[derive(serde::Serialize)];common.alarm.Status=#[derive(serde::Serialize)]"
```

A malformed pair (missing `=`) will stop the build with an error describing the text that was not parseable.

## Keepalive configuration

HTTP/2 keepalive probes are sent on every channel. The timing can be overridden at runtime via environment variables:

| Variable | Default | Meaning |
| -------- | ------- | ------- |
| `RUST_GRPC_LIB_KEEP_ALIVE_INTERVAL_SECS` | `30` | Seconds between keepalive pings |
| `RUST_GRPC_LIB_KEEP_ALIVE_TIMEOUT_SECS` | `10` | Seconds to wait for a keepalive ping acknowledgement before closing the connection |

Keepalive pings are also sent while the connection is idle.

## Development

This project uses a devcontainer. Open in VS Code with the Dev Containers extension installed and you will be prompted to reopen in the container, which has all required tools pre-installed.

```
# After cloning
git submodule update --init --recursive
cargo build
```
