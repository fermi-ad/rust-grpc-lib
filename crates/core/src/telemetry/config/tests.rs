use std::{
    collections::HashMap,
    fmt::Debug,
    sync::{Arc, Mutex},
};

use tracing::{Event, Level, Subscriber};
use tracing_subscriber::{Layer, layer::Context, prelude::*};

use super::{
    DEFAULT_DURATION_BUCKET_BOUNDS_SECONDS, Duration, LOG_TARGET, TelemetryConfig, valid_buckets,
};

const BASE_ENDPOINT: &str = "https://collector.example";
const METRICS_ENDPOINT: &str = "https://collector.example/v1/metrics";
const SIGNAL_ENDPOINT: &str = "https://collector.example/custom";
const BASE_VAR: &str = "OTEL_EXPORTER_OTLP_ENDPOINT";
const SIGNAL_VAR: &str = "OTEL_EXPORTER_OTLP_METRICS_ENDPOINT";

fn config(vars: &[(&str, &str)]) -> TelemetryConfig {
    let vars: HashMap<_, _> = vars.iter().copied().collect();
    TelemetryConfig::from_lookup(|key| vars.get(key).map(|value| (*value).to_owned()))
}

#[test]
fn defaults_match_an_empty_environment() {
    let executable = std::env::current_exe().unwrap();
    let file_name = executable.file_name().unwrap().to_string_lossy();
    for config in [TelemetryConfig::default(), config(&[])] {
        assert_eq!(config.export_endpoint(), None);
        assert!(!config.sdk_disabled);
        assert_eq!(config.export_interval, Duration::from_secs(10));
        assert_eq!(config.service_name, format!("unknown_service:{file_name}"));
        assert_eq!(config.service_version, None);
        assert_eq!(
            config.duration_buckets,
            DEFAULT_DURATION_BUCKET_BOUNDS_SECONDS
        );
        assert!(config.runtime_probe);
    }
}

#[test]
fn default_buckets_cover_six_decades_and_common_latency_budgets() {
    let expected = (-5..=0).flat_map(|power| {
        ["1", "1.5", "2", "3", "5", "7"]
            .map(|value| format!("{value}e{power}").parse::<f64>().unwrap())
    });
    assert!(
        DEFAULT_DURATION_BUCKET_BOUNDS_SECONDS
            .into_iter()
            .eq(expected)
    );
    assert!(valid_buckets(&DEFAULT_DURATION_BUCKET_BOUNDS_SECONDS));
    assert!(DEFAULT_DURATION_BUCKET_BOUNDS_SECONDS.contains(&0.015));
    assert!(DEFAULT_DURATION_BUCKET_BOUNDS_SECONDS.contains(&0.050));
    assert!(valid_buckets(&[0.01]));
    assert!(!valid_buckets(&[]));
}

#[test]
fn endpoints_use_signal_before_base_and_only_empty_is_unset() {
    for (vars, endpoint) in [
        (
            &[(BASE_VAR, BASE_ENDPOINT), (SIGNAL_VAR, SIGNAL_ENDPOINT)][..],
            Some(SIGNAL_ENDPOINT),
        ),
        (
            &[(BASE_VAR, BASE_ENDPOINT), (SIGNAL_VAR, "")][..],
            Some(METRICS_ENDPOINT),
        ),
        (
            &[(BASE_VAR, "https://collector.example/")][..],
            Some(METRICS_ENDPOINT),
        ),
        (&[(BASE_VAR, ""), (SIGNAL_VAR, "")][..], None),
        (
            &[(BASE_VAR, BASE_ENDPOINT), (SIGNAL_VAR, " ")][..],
            Some(" "),
        ),
    ] {
        assert_eq!(config(vars).export_endpoint(), endpoint);
    }
    assert_eq!(
        config(&[(BASE_VAR, BASE_ENDPOINT)])
            .endpoint("")
            .export_endpoint(),
        None
    );
}

#[test]
fn sdk_disable_takes_precedence_over_endpoint() {
    assert!(config(&[("OTEL_SDK_DISABLED", "true")]).sdk_disabled);
    for (value, disabled) in [
        ("TrUe", true),
        ("false", false),
        ("", false),
        ("1", false),
        (" true ", false),
    ] {
        let config = config(&[(BASE_VAR, BASE_ENDPOINT), ("OTEL_SDK_DISABLED", value)]);
        assert_eq!(
            config.export_endpoint(),
            (!disabled).then_some(METRICS_ENDPOINT)
        );
    }
}

#[test]
fn interval_requires_positive_milliseconds() {
    let var = "OTEL_METRIC_EXPORT_INTERVAL";
    assert_eq!(
        config(&[(var, "250")]).export_interval,
        Duration::from_millis(250)
    );
    for value in [
        "0",
        "",
        " 250 ",
        "-10",
        "1.5",
        "bad",
        "18446744073709551616",
    ] {
        assert_eq!(
            config(&[(var, value)]).export_interval,
            Duration::from_secs(10)
        );
    }
    assert_eq!(
        config(&[(var, "250")])
            .export_interval(Duration::ZERO)
            .export_interval,
        Duration::from_secs(10)
    );
}

#[test]
fn service_identity_uses_explicit_name_then_env_then_resource_then_executable() {
    let vars = [
        ("OTEL_SERVICE_NAME", "deployment"),
        (
            "OTEL_RESOURCE_ATTRIBUTES",
            "service.name=older, malformed, service.name = resource=name , service.version=ignored",
        ),
    ];
    assert_eq!(config(&vars).service_name, "deployment");
    assert_eq!(
        config(&vars).service_name("explicit").service_name,
        "explicit"
    );
    assert_eq!(config(&vars[1..]).service_name, "resource=name");
    assert_eq!(
        config(&[("OTEL_SERVICE_NAME", ""), vars[1]]).service_name,
        "resource=name"
    );
    assert_eq!(config(&vars).service_version, None);
    assert_eq!(
        config(&vars)
            .service_version("build-42")
            .service_version
            .as_deref(),
        Some("build-42")
    );
    let fallback = TelemetryConfig::default().service_name;
    for attributes in [
        "",
        "service.namespace=ns",
        "service.name = ",
        "service.name",
    ] {
        assert_eq!(
            config(&[("OTEL_RESOURCE_ATTRIBUTES", attributes)]).service_name,
            fallback
        );
    }
    assert_eq!(config(&vars).service_name("").service_name, fallback);
}

#[test]
fn explicit_setters_override_the_environment_snapshot() {
    let config = config(&[
        (SIGNAL_VAR, SIGNAL_ENDPOINT),
        ("OTEL_SDK_DISABLED", "true"),
        ("OTEL_METRIC_EXPORT_INTERVAL", "250"),
        ("OTEL_SERVICE_NAME", "deployment"),
        ("GRPC_DURATION_BUCKETS", "0.01, 1.5e-2 ,0.050"),
        ("GRPC_RUNTIME_PROBE", "FaLsE"),
    ]);
    assert_eq!(config.duration_buckets, [0.01, 0.015, 0.05]);
    assert!(!config.runtime_probe);
    let config = config
        .endpoint(METRICS_ENDPOINT)
        .sdk_disabled(false)
        .export_interval(Duration::from_secs(2))
        .service_name("explicit")
        .service_version("42")
        .duration_bucket_bounds_seconds([0.025, 0.1])
        .runtime_scheduling_probe(true);
    assert_eq!(config.export_endpoint(), Some(METRICS_ENDPOINT));
    assert_eq!(config.export_interval, Duration::from_secs(2));
    assert_eq!(config.service_name, "explicit");
    assert_eq!(config.service_version.as_deref(), Some("42"));
    assert_eq!(config.duration_buckets, [0.025, 0.1]);
    assert!(config.runtime_probe);
    assert!(!config.runtime_scheduling_probe(false).runtime_probe);
}

#[test]
fn invalid_buckets_and_probe_warn_without_echoing_input() {
    #[derive(Clone, Default)]
    struct Warnings(Arc<Mutex<Vec<String>>>);

    impl<S: Subscriber> Layer<S> for Warnings {
        fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
            let metadata = event.metadata();
            assert_eq!(metadata.target(), LOG_TARGET);
            assert_eq!(*metadata.level(), Level::WARN);
            event.record(&mut |field: &tracing::field::Field, value: &dyn Debug| {
                assert_eq!(field.name(), "message");
                self.0.lock().unwrap().push(format!("{value:?}"));
            });
        }
    }

    let warnings = Warnings::default();
    let subscriber = tracing_subscriber::registry().with(warnings.clone());
    tracing::subscriber::with_default(subscriber, || {
        for value in [
            "",
            "0.01,",
            "0.01,not-a-number",
            "NaN",
            "inf",
            "0",
            "-1",
            "2,2",
            "2,1",
        ] {
            assert_eq!(
                config(&[("GRPC_DURATION_BUCKETS", value)]).duration_buckets,
                DEFAULT_DURATION_BUCKET_BOUNDS_SECONDS
            );
        }
        assert_eq!(
            TelemetryConfig::default()
                .duration_bucket_bounds_seconds([0.01])
                .duration_bucket_bounds_seconds([0.0])
                .duration_buckets,
            DEFAULT_DURATION_BUCKET_BOUNDS_SECONDS
        );
        for value in ["", "1", "off", " true "] {
            assert!(config(&[("GRPC_RUNTIME_PROBE", value)]).runtime_probe);
        }
        assert!(config(&[("GRPC_RUNTIME_PROBE", "TrUe")]).runtime_probe);
    });
    let warnings = warnings.0.lock().unwrap();
    assert_eq!(warnings.len(), 14);
    assert!(
        warnings[..10]
            .iter()
            .all(|msg| msg == "invalid duration buckets; using defaults")
    );
    assert!(
        warnings[10..]
            .iter()
            .all(|msg| msg == "invalid GRPC_RUNTIME_PROBE; using default")
    );
}

#[test]
fn debug_does_not_show_endpoint_credentials_or_read_headers() {
    let config = TelemetryConfig::from_lookup(|key| match key {
        SIGNAL_VAR => Some("https://user:password@collector.example/metrics?key=secret".into()),
        "OTEL_EXPORTER_OTLP_HEADERS" | "OTEL_EXPORTER_OTLP_METRICS_HEADERS" => {
            panic!("unexpected header lookup")
        }
        _ => None,
    });
    let debug = format!("{config:?}");
    assert!(debug.contains("endpoint_configured: true"));
    for value in ["user", "password", "secret", "collector.example"] {
        assert!(!debug.contains(value));
    }
}
