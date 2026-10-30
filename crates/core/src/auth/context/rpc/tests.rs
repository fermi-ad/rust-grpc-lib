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
