//! SIGTERM changes the process's signal handlers; run it in its own binary.

#![cfg(unix)]

mod support;

#[tokio::test]
async fn sigterm_drains_the_server() {
    support::assert_graceful_shutdown("-TERM").await;
}
