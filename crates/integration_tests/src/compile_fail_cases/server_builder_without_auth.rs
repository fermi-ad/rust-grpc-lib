use rust_grpc_lib::server::GrpcServer;

fn main() {
    // No authentication mode was chosen, so services cannot be added.
    let _ = GrpcServer::builder().add_service(());
}
