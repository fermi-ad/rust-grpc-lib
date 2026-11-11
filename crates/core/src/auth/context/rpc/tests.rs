//! Tests for [`super::RpcMethod`].

use super::*;

#[test]
fn rpc_method_parses_a_grpc_path() {
    assert_eq!(
        RpcMethod::from_path("/services.example.ExampleDB/getEndPointOne"),
        Some(RpcMethod {
            service: "services.example.ExampleDB".into(),
            method: "getEndPointOne".into(),
        })
    );
}

#[test]
fn a_known_method_that_disagrees_with_the_path_is_ignored() {
    let known = RpcMethod {
        service: "pkg.Service".into(),
        method: "Other".into(),
    };

    let parsed = RpcMethod::from_path_reusing("/pkg.Service/Do", Some(known)).unwrap();

    assert_eq!(parsed.method, "Do");
}

#[test]
fn a_known_method_does_not_rescue_a_malformed_path() {
    let known = RpcMethod {
        service: "pkg.Service".into(),
        method: "Do".into(),
    };

    assert_eq!(RpcMethod::from_path_reusing("/a/b/c", Some(known)), None);
}

#[test]
fn rpc_method_rejects_malformed_paths() {
    for path in [
        "",
        "/",
        "//",
        "/Service",
        "/Service/",
        "//Method",
        "Service/Method",
        "/a/b/c",
    ] {
        assert_eq!(RpcMethod::from_path(path), None, "path {path:?}");
    }
}
