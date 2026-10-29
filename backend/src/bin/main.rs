//! Executable entry point: logging, configuration, startup and the HTTP server.
//! Everything else lives in the library crate.
//!
//! Usage: `portfolio-investigation [CONFIG_FILE]`. Without an argument the
//! configuration is read from `config.toml` in the working directory.

#![forbid(unsafe_code)]

use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;

use portfolio_investigation::config::Config;
use portfolio_investigation::error::AppError;
use portfolio_investigation::utils::shutdown::shutdown_signal;
use portfolio_investigation::{build_router, initialise};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,tower_http=info,sqlx=warn")),
        )
        // Colours only on a terminal; `docker logs` and log files get plain text.
        .with_ansi(std::io::stdout().is_terminal())
        .init();

    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // Each error's message includes its cause, so this one line
            // carries the whole chain (path, row, underlying I/O error...).
            tracing::error!("{error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), AppError> {
    let config_path = std::env::args_os().nth(1).map(PathBuf::from);
    let config = Config::load(config_path.as_deref(), None)?;

    tracing::info!(?config, "configuration loaded");

    let state = initialise(&config).await?;

    if config.static_dir().is_none() {
        tracing::info!("API-only mode (no static_dir)");
    }

    let app = build_router(state, config.static_dir());

    let addr = config.bind_addr();
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|source| AppError::Bind { addr, source })?;
    let local_addr = listener
        .local_addr()
        .map_err(|source| AppError::Bind { addr, source })?;

    tracing::info!("listening on http://{local_addr}");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(AppError::Serve)?;

    tracing::info!("server stopped");
    Ok(())
}
