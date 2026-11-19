use super::opentelemetry::metrics::{MeterProvider, noop::NoopMeterProvider};

#[test]
fn reexport_exposes_metrics_api() {
    let meter = NoopMeterProvider::new().meter("telemetry-test");
    meter.u64_counter("test.calls").build().add(1, &[]);
}
