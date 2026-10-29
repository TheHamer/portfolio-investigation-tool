//! The graceful-shutdown future must stay pending until a signal arrives,
//! then resolve. Runs in its own test binary because it signals the process.

#![cfg(unix)]

use std::time::Duration;

use portfolio_investigation::utils::shutdown::shutdown_signal;

#[tokio::test]
async fn shutdown_waits_for_sigterm() {
    let signal = tokio::spawn(shutdown_signal());

    // Give the future time to be polled and register its handlers. It must
    // not resolve on its own (an early return would stop the server at once).
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!signal.is_finished(), "shutdown resolved without a signal");

    // SIGTERM is what `docker stop` sends. Tokio's handler is installed, so
    // this does not kill the test process; it completes the future instead.
    let status = std::process::Command::new("kill")
        .args(["-TERM", &std::process::id().to_string()])
        .status()
        .expect("run kill");
    assert!(status.success());

    tokio::time::timeout(Duration::from_secs(5), signal)
        .await
        .expect("shutdown future resolves after SIGTERM")
        .unwrap();
}
