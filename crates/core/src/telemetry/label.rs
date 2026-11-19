//! Closed-set method labels built from registered wire names and access policies.

use std::collections::HashMap;

use crate::auth::{PolicyTable, policy::same_method};

use super::LOG_TARGET;

#[cfg(test)]
mod tests;

const HEALTH_METHODS: [&str; 2] = ["grpc.health.v1.Health/Check", "grpc.health.v1.Health/Watch"];

pub(crate) struct MethodLabels {
    pub(crate) service_name: &'static str,
    pub(crate) wire_methods: &'static [&'static str],
    pub(crate) table: PolicyTable,
}

/// Maps exact canonical paths to the static labels recorded at registration.
pub(crate) struct LabelIndex {
    labels: HashMap<&'static str, &'static str>,
}

impl LabelIndex {
    pub(crate) fn new() -> Self {
        Self {
            labels: HEALTH_METHODS
                .into_iter()
                .map(|wire| (wire, wire))
                .collect(),
        }
    }

    pub(crate) fn register(&mut self, service: MethodLabels) {
        // Normalize only when pairing trusted metadata with the policy table,
        // never when looking up a request path.
        let methods = service.wire_methods.iter().copied().filter_map(|wire| {
            let (name, method) = wire.split_once('/')?;
            (name == service.service_name && !method.is_empty() && !method.contains('/'))
                .then_some((wire, method))
        });
        for policy in service.table.policies() {
            let mut matches = methods
                .clone()
                .filter(|(_, method)| same_method(policy.method, method));
            match (matches.next(), matches.next()) {
                (Some((wire, _)), None) => {
                    self.labels.insert(wire, wire);
                }
                (first, _) => {
                    tracing::warn!(
                        target: LOG_TARGET,
                        service = service.service_name,
                        policy_method = policy.method,
                        reason = if first.is_none() { "missing" } else { "ambiguous" },
                        "policy does not identify exactly one wire method; omitting telemetry label"
                    );
                }
            }
        }
    }

    /// The complete set of static labels known at layer construction.
    pub(super) fn methods(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.labels.values().copied()
    }

    /// Resolve an exact URI path. The label always comes from the index, not
    /// from the request. Unlisted paths return `None` (reported as `_OTHER`).
    pub(crate) fn resolve(&self, path: &str) -> Option<&'static str> {
        self.labels.get(path.strip_prefix('/')?).copied()
    }
}
