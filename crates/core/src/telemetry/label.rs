//! Closed-set method labels built from registered wire names and access policies.

use std::collections::HashSet;

use crate::auth::{PolicyTable, policy::same_method};

use super::LOG_TARGET;

#[cfg(test)]
mod tests;

const HEALTH_METHODS: [&str; 2] = ["grpc.health.v1.Health/Check", "grpc.health.v1.Health/Watch"];

pub(crate) struct MethodLabelIndex {
    labels: HashSet<&'static str>,
}

impl MethodLabelIndex {
    pub(crate) fn new() -> Self {
        Self {
            labels: HashSet::from(HEALTH_METHODS),
        }
    }

    pub(crate) fn register(
        &mut self,
        service_name: &'static str,
        wire_methods: &'static [&'static str],
        table: PolicyTable,
    ) {
        // Normalize only when pairing trusted metadata with the policy table,
        // never when looking up a request path.
        let methods = wire_methods.iter().copied().filter_map(|wire| {
            let (name, method) = wire.split_once('/')?;
            (name == service_name && !method.is_empty() && !method.contains('/'))
                .then_some((wire, method))
        });
        for policy in table.policies() {
            let mut matches = methods
                .clone()
                .filter(|(_, method)| same_method(policy.method, method));
            match (matches.next(), matches.next()) {
                (Some((wire, _)), None) => {
                    self.labels.insert(wire);
                }
                (first, _) => {
                    tracing::warn!(
                        target: LOG_TARGET,
                        service = service_name,
                        policy_method = policy.method,
                        reason = if first.is_none() { "missing" } else { "ambiguous" },
                        "policy does not identify exactly one wire method; omitting telemetry label"
                    );
                }
            }
        }
    }

    /// Never returns request-derived text, so label cardinality stays bounded.
    pub(crate) fn label_for_path(&self, path: &str) -> Option<&'static str> {
        self.labels.get(path.strip_prefix('/')?).copied()
    }
}
