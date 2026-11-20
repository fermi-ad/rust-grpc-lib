//! Process signal registration shared by the serving and standalone APIs.

use std::future::Future;
use std::io;

/// Waits for SIGTERM or SIGINT on Unix, or Ctrl-C on Windows. Registration
/// happens when the future is first polled.
///
/// On Unix, Tokio's handlers replace the default behavior for these signals
/// for the rest of the process, even after this future completes or is dropped.
///
/// # Panics
///
/// On Unix, requires a Tokio runtime with I/O enabled (`enable_all()` suffices).
pub async fn shutdown_signal() -> io::Result<()> {
    register()?.await;
    Ok(())
}

// Register synchronously so the serving path can fail before binding, and
// notifications during startup are already captured by the listeners.
#[cfg(unix)]
pub(super) fn register() -> io::Result<impl Future<Output = ()>> {
    use tokio::signal::unix::{SignalKind, signal};

    let mut terminate = signal(SignalKind::terminate())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    Ok(async move {
        let signal = tokio::select! {
            _ = terminate.recv() => "SIGTERM",
            _ = interrupt.recv() => "SIGINT",
        };
        tracing::info!(signal, "shutdown signal received");
    })
}

#[cfg(not(unix))]
pub(super) fn register() -> io::Result<impl Future<Output = ()>> {
    // Use the eager console listener rather than the portable async wrapper,
    // which would defer registration and its error until after binding.
    let mut interrupt = tokio::signal::windows::ctrl_c()?;
    Ok(async move {
        interrupt.recv().await;
        tracing::info!(signal = "Ctrl-C", "shutdown signal received");
    })
}
