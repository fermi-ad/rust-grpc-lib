use super::{DEFAULT_DURATION_BUCKETS, validate_buckets};

#[test]
fn defaults_cover_six_decades_and_common_latency_budgets() {
    let expected = (-5..=0).flat_map(|power| {
        ["1", "1.5", "2", "3", "5", "7"]
            .map(|value| format!("{value}e{power}").parse::<f64>().unwrap())
    });
    assert_eq!(DEFAULT_DURATION_BUCKETS.len(), 36);
    assert!(DEFAULT_DURATION_BUCKETS.into_iter().eq(expected));
    assert!(validate_buckets(&DEFAULT_DURATION_BUCKETS));
    assert!(DEFAULT_DURATION_BUCKETS.contains(&0.015));
    assert!(DEFAULT_DURATION_BUCKETS.contains(&0.050));
}

#[test]
fn validation_rejects_every_invalid_shape() {
    for bounds in [
        &[][..],
        &[f64::NAN],
        &[f64::INFINITY],
        &[f64::NEG_INFINITY],
        &[0.0],
        &[-0.01],
        &[0.01, 0.01],
        &[0.02, 0.01],
    ] {
        assert!(!validate_buckets(bounds), "accepted {bounds:?}");
    }
    assert!(validate_buckets(&[0.01]));
    assert!(validate_buckets(&[0.01, 0.02]));
}
