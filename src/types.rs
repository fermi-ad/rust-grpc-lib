//! Protobuf types for all Controls gRPC services.
//!
//! The types themselves are generated at build time from the `.proto`
//! definitions in the `extern/` submodule — do not edit the generated code
//! directly. However, the **module declarations** in this file must be kept in
//! sync with the set of services defined in `extern/`. When a new version of
//! the submodule is integrated, follow these steps:
//!
//! 1. Update the submodule: `git submodule update --remote extern`
//! 2. Run `cargo build` to regenerate the compiled proto output.
//! 3. For each **new** `.proto` service file added to `extern/`:
//!    - Add a corresponding `pub mod <name> { tonic::include_proto!("<package>"); }`
//!      entry inside the appropriate parent module in this file.
//!    - Add a row to the service table in the crate-level docs in [`lib.rs`](crate).
//! 4. For each **removed** service:
//!    - Remove or `#[deprecated]`-mark the corresponding module entry.
//!    - Remove its row from the service table in [`lib.rs`](crate).
//! 5. Commit both the updated `extern` pointer and the changes to this file together.
//!
//! The `<package>` string passed to `tonic::include_proto!` must match the
//! `package` declaration at the top of the `.proto` file, with `.` used as the
//! separator (e.g. `services.alarm_commands`).
//!
//! # Module layout
//!
//! | Module | Contents |
//! |---|---|
//! | [`common`] | Shared message types used across multiple services (alarms, devices, DRF, events, sources, status) |
//! | [`services`] | Per-service request/response types and generated tonic client structs |
//! | [`google::protobuf`] | Well-known protobuf types (`Timestamp`, `Duration`, etc.) |
//! | [`third_party`] | Third-party proto types vendored alongside the Controls protos |
//!
//! # Finding a service client
//!
//! Each service module contains a nested `*_client` module with the tonic
//! client struct. For example:
//!
//! ```rust,no_run
//! use rust_grpc_lib::types::services::alarm_commands::alarm_commands_client::AlarmCommandsClient;
//! ```
//!
//! Register the client with [`register_client!`](crate::register_client) before
//! passing it to [`pool::client`](crate::pool::client).

pub mod common {
    pub mod alarm {
        tonic::include_proto!("common.alarm");
    }
    pub mod device {
        tonic::include_proto!("common.device");
    }
    pub mod drf {
        tonic::include_proto!("common.drf");
    }
    pub mod event {
        tonic::include_proto!("common.event");
    }
    pub mod sources {
        tonic::include_proto!("common.sources");
    }
    pub mod status {
        tonic::include_proto!("common.status");
    }
}

#[deprecated = "Old way of calling DPM. `services::daq` is the replacement in most cases."]
pub mod dpm {
    tonic::include_proto!("dpm");
}

pub mod google {
    pub mod protobuf {
        tonic::include_proto!("google.protobuf");
    }
}

pub mod services {
    pub mod alarm_commands {
        tonic::include_proto!("services.alarm_commands");
    }
    pub mod alarm_groups {
        tonic::include_proto!("services.alarm_groups");
    }
    pub mod alarm_timers {
        tonic::include_proto!("services.alarm_timers");
    }
    pub mod alarm_user_layouts {
        tonic::include_proto!("services.alarm_user_layouts");
    }
    pub mod clock_event {
        tonic::include_proto!("services.clock_event");
    }
    pub mod daq {
        tonic::include_proto!("services.daq");
    }
    pub mod devdb {
        tonic::include_proto!("services.devdb");
    }
    /// The `example` module gets used by the [`grpc-db-template`](https://github.com/fermi-ad/grpc-db-template) repo.
    /// It is included here for completeness, so the template can use this library out of the gate.
    #[doc(hidden)]
    pub mod example {
        tonic::include_proto!("services.example");
    }
    pub mod ioc_alarms {
        tonic::include_proto!("services.ioc_alarms");
    }
    pub mod tlg_placement {
        tonic::include_proto!("services.tlg_placement");
    }
}

pub mod third_party {
    pub mod r#type {
        tonic::include_proto!("third_party.r#type");
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashSet, ffi::OsStr, fs::read_dir, path::Path};

    /// The set of proto packages declared via `tonic::include_proto!` in this
    /// file. When you add or remove a module above, update this list to match.
    ///
    /// `google.protobuf` is intentionally absent — it is compiled from the
    /// well-known types bundled with `tonic_prost_build`, not from `extern/`.
    const DECLARED_PACKAGES: &[&str] = &[
        "common.alarm",
        "common.device",
        "common.drf",
        "common.event",
        "common.sources",
        "common.status",
        "dpm",
        "services.alarm_commands",
        "services.alarm_groups",
        "services.alarm_timers",
        "services.alarm_user_layouts",
        "services.clock_event",
        "services.daq",
        "services.devdb",
        "services.example",
        "services.ioc_alarms",
        "services.tlg_placement",
        "third_party.type",
    ];

    fn collect_proto_packages(dir: &Path) -> HashSet<String> {
        let mut packages = HashSet::new();
        let Ok(entries) = read_dir(dir) else {
            return packages;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                packages.extend(collect_proto_packages(&path));
            } else if path.extension() == Some(OsStr::new("proto")) {
                if let Ok(contents) = std::fs::read_to_string(&path) {
                    for line in contents.lines() {
                        let trimmed = line.trim();
                        if let Some(rest) = trimmed.strip_prefix("package ") {
                            let pkg = rest.trim_end_matches(';').trim().to_string();
                            if !pkg.is_empty() {
                                packages.insert(pkg);
                            }
                            break;
                        }
                    }
                }
            }
        }
        packages
    }

    #[test]
    fn declared_packages_match_extern_protos() {
        let extern_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("extern");
        let on_disk = collect_proto_packages(&extern_dir);
        let declared: HashSet<&str> = DECLARED_PACKAGES.iter().copied().collect();

        let missing: Vec<_> = on_disk
            .iter()
            .filter(|p| !declared.contains(p.as_str()))
            .collect();
        let extra: Vec<_> = declared.iter().filter(|p| !on_disk.contains(**p)).collect();

        assert!(
            missing.is_empty() && extra.is_empty(),
            "src/types.rs is out of sync with extern/:\n\
             Packages in extern/ but not declared in types.rs: {missing:?}\n\
             Packages declared in types.rs but not found in extern/: {extra:?}\n\
             \n\
             Add or remove the corresponding `pub mod` entries in src/types.rs \
             and update DECLARED_PACKAGES in this test."
        );
    }
}
