use std::collections::HashSet;
use std::sync::Mutex;

use tracing::{Event, Level, Subscriber};
use tracing_subscriber::{Layer, layer::Context, prelude::*};

use crate::auth::{Access, MethodPolicy, ServicePolicy};

use super::{HEALTH_METHODS, LOG_TARGET, MethodLabelIndex, PolicyTable};

const WIRE_METHODS: &[&str] = &[
    "pkg.Service/getEndPointOne",
    "pkg.Service/GetBPMList",
    "pkg.Service/NoPolicy",
];

struct Service;

impl ServicePolicy<()> for Service {
    const POLICIES: &'static [MethodPolicy] = &[
        MethodPolicy::new("get_end_point_one", Access::Public),
        MethodPolicy::new("r#get_bpm_list", Access::Authenticated),
    ];
}

fn index() -> MethodLabelIndex {
    let mut index = MethodLabelIndex::new();
    index.register(
        "pkg.Service",
        WIRE_METHODS,
        PolicyTable::of::<(), Service>(),
    );
    index
}

#[test]
fn only_exact_registered_paths_with_policies_resolve() {
    let index = index();
    // Use owned request data and verify that the returned label points into
    // the wire metadata, not the request.
    for wire in &WIRE_METHODS[..2] {
        let path = format!("/{wire}");
        let label = index.label_for_path(&path).unwrap();
        assert_eq!(label, *wire);
        assert_eq!(label.as_ptr(), wire.as_ptr());
    }
    for path in [
        "pkg.Service/getEndPointOne",
        "/pkg.Service/GETENDPOINTONE",
        "/pkg.Service/get_end_point_one",
        "/pkg.Service/r#getEndPointOne",
        "/pkg.Service/get%45ndPointOne",
        "/pkg.service/getEndPointOne",
        "/pkg.Unknown/getEndPointOne",
        "//pkg.Service/getEndPointOne",
        "/pkg.Service//getEndPointOne",
        "/pkg.Service/getEndPointOne/",
        "/pkg.Service/",
        "//getEndPointOne",
        "/",
        "",
        "/pkg.Service/NoPolicy",
        "/grpc.reflection.v1.ServerReflection/ServerReflectionInfo",
    ] {
        assert_eq!(index.label_for_path(path), None, "accepted {path:?}");
    }
}

#[test]
fn health_paths_resolve_without_a_policy_table() {
    let index = MethodLabelIndex::new();
    assert_eq!(
        index.label_for_path("/grpc.health.v1.Health/Check"),
        Some(HEALTH_METHODS[0])
    );
    assert_eq!(
        index.label_for_path("/grpc.health.v1.Health/Watch"),
        Some(HEALTH_METHODS[1])
    );
    assert_eq!(index.label_for_path("/grpc.health.v1.Health/check"), None);
    assert_eq!(index.label_for_path("/grpc.health.v1.Health/List"), None);
}

#[test]
fn missing_and_ambiguous_policies_warn_at_registration_only() {
    struct CountWarnings(Mutex<usize>);

    impl<S: Subscriber> Layer<S> for CountWarnings {
        fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
            let meta = event.metadata();
            if meta.target() == LOG_TARGET && *meta.level() == Level::WARN {
                *self.0.lock().unwrap() += 1;
            }
        }
    }

    let subscriber = tracing_subscriber::registry().with(CountWarnings(Mutex::new(0)));
    tracing::subscriber::with_default(subscriber, || {
        let warning_count = || {
            tracing::dispatcher::get_default(|dispatch| {
                *dispatch
                    .downcast_ref::<CountWarnings>()
                    .unwrap()
                    .0
                    .lock()
                    .unwrap()
            })
        };
        let mut index = index();
        index.register(
            "pkg.Other",
            &[
                "pkg.Other/getEndPointOne",
                "pkg.Other/GetEndPointOne",
                "pkg.Foreign/GetBPMList",
            ],
            PolicyTable::of::<(), Service>(),
        );
        assert_eq!(warning_count(), 2, "one missing and one ambiguous policy");
        for path in [
            "/pkg.Other/getEndPointOne",
            "/pkg.Other/GetEndPointOne",
            "/pkg.Foreign/GetBPMList",
        ] {
            assert_eq!(index.label_for_path(path), None);
        }
        assert_eq!(
            index.label_for_path("/pkg.Service/getEndPointOne"),
            Some(WIRE_METHODS[0])
        );
        assert_eq!(warning_count(), 2, "no additional warnings during lookup");
    });
}

#[test]
fn arbitrary_paths_cannot_expand_the_label_set() {
    let index = index();
    let mut labels = HashSet::new();
    for n in 0..1000 {
        let path = match n % 5 {
            0 => format!("/pkg.Service/getEndPointOne{n}"),
            1 => format!("/pkg.Unknown{n}/GetBPMList"),
            2 => format!("/grpc.health.v1.Health/Check/{n}"),
            3 => format!("/pkg.Service/{n:x}"),
            _ => format!("//{n}/"),
        };
        labels.insert(index.label_for_path(&path));
    }
    assert_eq!(labels, HashSet::from([None]));
    for wire in WIRE_METHODS.iter().chain(&HEALTH_METHODS) {
        labels.insert(index.label_for_path(&format!("/{wire}")));
    }
    assert!(labels.len() <= WIRE_METHODS.len() + HEALTH_METHODS.len() + 1);
}
