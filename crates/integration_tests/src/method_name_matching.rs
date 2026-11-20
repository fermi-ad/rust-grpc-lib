//! Macro-emitted method names against the gate's runtime lookup: each Rust
//! handler name must be found by `PolicyTable::lookup` under its proto wire
//! name. Guards the duplicated normalization in the macro (`normalize`,
//! `crates/grpc_macro/src/service/tests.rs`) and in core (`same_method`,
//! `crates/core/src/auth/policy/tests.rs`); keep the cases in the three
//! places in step.
//!
//! The macro emits the Rust name minus `r#`; the gate normalizes at lookup.
//! Every method has a distinct policy, so a lookup that lands on the wrong
//! entry fails the comparison.

use rust_grpc_lib::access_controlled_service;
use rust_grpc_lib::auth::{Access, PolicyTable, PublicContext, RequestContext};
use rust_grpc_lib::tonic::{Request, Response, Status};

#[allow(dead_code)]
trait Spelling {
    async fn r#type(&self, request: Request<String>) -> Result<Response<String>, Status>;
    async fn get_bpm_list(&self, request: Request<String>) -> Result<Response<String>, Status>;
    async fn get_v2_data(&self, request: Request<String>) -> Result<Response<String>, Status>;
    async fn ping(&self, request: Request<String>) -> Result<Response<String>, Status>;
    async fn read_io(&self, request: Request<String>) -> Result<Response<String>, Status>;
}

struct SpellingImpl;

/// Stands in for a generated `FooServer<T>`, which keys the table.
struct SpellingServerStandIn<T>(#[allow(dead_code)] T);

#[allow(clippy::unused_async_trait_impl)]
#[access_controlled_service(server = SpellingServerStandIn)]
impl Spelling for SpellingImpl {
    #[roles(any("type-role"))]
    async fn r#type(&self, ctx: RequestContext<String>) -> Result<Response<String>, Status> {
        Ok(Response::new(ctx.message))
    }

    #[roles(all("bpm-role"))]
    async fn get_bpm_list(&self, ctx: RequestContext<String>) -> Result<Response<String>, Status> {
        Ok(Response::new(ctx.message))
    }

    #[roles(any("v2-role"))]
    async fn get_v2_data(&self, ctx: RequestContext<String>) -> Result<Response<String>, Status> {
        Ok(Response::new(ctx.message))
    }

    #[public]
    async fn ping(&self, ctx: PublicContext<String>) -> Result<Response<String>, Status> {
        Ok(Response::new(ctx.message))
    }

    #[any_authenticated]
    async fn read_io(&self, ctx: RequestContext<String>) -> Result<Response<String>, Status> {
        Ok(Response::new(ctx.message))
    }
}

fn table() -> PolicyTable {
    PolicyTable::for_service(SpellingServerStandIn::<SpellingImpl>)
}

/// (Rust name as written, proto wire name, the policy it declared).
fn cases() -> [(&'static str, &'static str, Access); 5] {
    [
        ("r#type", "Type", Access::any_of(&["type-role"])),
        ("get_bpm_list", "GetBPMList", Access::all_of(&["bpm-role"])),
        ("get_v2_data", "GetV2Data", Access::any_of(&["v2-role"])),
        ("ping", "Ping", Access::Public),
        ("read_io", "ReadIO", Access::Authenticated),
    ]
}

#[test]
fn every_rust_name_is_found_by_its_proto_wire_name() {
    let table = table();

    for (rust, wire, access) in cases() {
        assert_eq!(table.lookup(wire), Some(access), "{rust} <-> {wire}");
    }
}
