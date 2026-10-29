//! Graceful-shutdown signal for `axum::serve(...).with_graceful_shutdown`.

use std::future::pending;

/// Resolves when the process is asked to stop: Ctrl+C on every platform, and
/// SIGTERM on Unix (what `docker stop` sends to the container's main process).
///
/// If a handler cannot be registered, the error is logged and that branch
/// waits forever instead of completing. Completing would be read by
/// `with_graceful_shutdown` as a shutdown request, stopping the server the
/// moment it started.
pub async fn shutdown_signal() {
    let ctrl_c = async {
        match tokio::signal::ctrl_c().await {
            Ok(()) => tracing::info!("received Ctrl+C"),
            Err(error) => {
                tracing::error!(%error, "could not listen for Ctrl+C");
                pending::<()>().await;
            }
        }
    };

    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut sigterm) => {
                sigterm.recv().await;
                tracing::info!("received SIGTERM");
            }
            Err(error) => {
                tracing::error!(%error, "could not install the SIGTERM handler");
                pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
    tracing::info!("shutting down: finishing in-flight requests");
}
