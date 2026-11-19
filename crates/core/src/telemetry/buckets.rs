//! Default duration boundaries and shared validation.

#[cfg(test)]
mod tests;

/// Default duration histogram boundaries in seconds, from 10 µs to 7 s.
///
/// Each decade uses the multipliers 1, 1.5, 2, 3, 5, and 7. The boundaries
/// include 15 ms and 50 ms for common latency budgets.
pub const DEFAULT_DURATION_BUCKETS: [f64; 36] = [
    1e-5, 1.5e-5, 2e-5, 3e-5, 5e-5, 7e-5, 1e-4, 1.5e-4, 2e-4, 3e-4, 5e-4, 7e-4, 1e-3, 1.5e-3, 2e-3,
    3e-3, 5e-3, 7e-3, 1e-2, 1.5e-2, 2e-2, 3e-2, 5e-2, 7e-2, 1e-1, 1.5e-1, 2e-1, 3e-1, 5e-1, 7e-1,
    1.0, 1.5, 2.0, 3.0, 5.0, 7.0,
];

pub(super) fn validate_buckets(bounds: &[f64]) -> bool {
    !bounds.is_empty()
        && bounds.iter().all(|&bound| bound.is_finite() && bound > 0.0)
        && bounds.windows(2).all(|pair| pair[0] < pair[1])
}
