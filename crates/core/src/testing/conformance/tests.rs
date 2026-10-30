//! Tests for the path validation of the conformance helpers.
//!
//! The helpers' behavior against a live server is covered in the
//! `integration_tests` crate, which has generated services to run them on.

use super::*;

#[test]
fn well_formed_paths_are_accepted() {
    check_paths(&[
        "/services.example.v1.ExampleDB/GetEndPointOne",
        "/pkg.Svc/Method",
    ])
    .expect("valid paths");
}

#[test]
fn empty_list_is_rejected() {
    let err = check_paths(&[]).unwrap_err();

    assert!(err.contains("no RPC paths"), "{err}");
}

#[test]
fn health_paths_are_rejected() {
    for path in [
        "/grpc.health.v1.Health/Check",
        "/grpc.health.v1.Health/Watch",
    ] {
        let err = check_paths(&["/pkg.Svc/Method", path]).unwrap_err();

        assert!(err.contains("open service"), "{err}");
        assert!(err.contains(path), "{err}");
    }
}

#[test]
fn reflection_paths_are_rejected() {
    let err =
        check_paths(&["/grpc.reflection.v1.ServerReflection/ServerReflectionInfo"]).unwrap_err();

    assert!(err.contains("open service"), "{err}");
}

#[test]
fn malformed_paths_are_rejected() {
    for path in [
        "",
        "/",
        "pkg.Svc/Method",
        "/pkg.Svc",
        "/pkg.Svc/",
        "//Method",
        "/pkg.Svc/Method/Extra",
        "/pkg.Svc/Met hod",
    ] {
        let err = check_paths(&[path]).unwrap_err();

        assert!(err.contains("not of the form"), "{path:?}: {err}");
    }
}
