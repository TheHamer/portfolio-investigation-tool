//! Application-level error: everything that can fail at startup or while
//! serving a request, as typed variants of the subsystem errors.
//!
//! `AppError` knows nothing about HTTP. The API layer wraps it in
//! `api::ApiError`, which alone decides status codes and response bodies.
//!
//! Convention: every error's message includes its cause's message, so
//! logging `{error}` once prints the whole chain (for example the CSV file,
//! row and field that failed). `#[source]` keeps the chain inspectable too.

use std::net::SocketAddr;

use thiserror::Error;

use crate::config::ConfigError;
use crate::import::ImportError;
use crate::manifest::ManifestError;
use crate::persistence::PersistenceError;
use crate::portfolio::PortfolioError;
use crate::reference::ReferenceError;

#[derive(Debug, Error)]
pub enum AppError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    #[error(transparent)]
    Reference(#[from] ReferenceError),
    #[error(transparent)]
    Persistence(#[from] PersistenceError),
    #[error(transparent)]
    Import(#[from] ImportError),
    #[error(transparent)]
    Portfolio(#[from] PortfolioError),
    #[error("could not listen on {addr}: {source}")]
    Bind {
        addr: SocketAddr,
        #[source]
        source: std::io::Error,
    },
    #[error("HTTP server failed: {0}")]
    Serve(#[source] std::io::Error),
}
