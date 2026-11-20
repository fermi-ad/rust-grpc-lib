//! SIGINT changes the process's signal handlers; run it in its own binary.

#![cfg(unix)]

mod support;

#[tokio::test]
async fn sigint_drains_the_server() {
    support::assert_graceful_shutdown("-INT").await;
}
