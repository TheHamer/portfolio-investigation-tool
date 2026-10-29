//! Errors from SQLite access, migrations and stored-row conversion.

use std::path::PathBuf;

use thiserror::Error;

use crate::domain::FieldError;

#[derive(Debug, Error)]
pub enum PersistenceError {
    #[error("could not create database directory {path}: {source}")]
    CreateDirectory {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not open database {path}: {source}")]
    Open {
        path: PathBuf,
        #[source]
        source: sqlx::Error,
    },
    #[error("database migration failed: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),
    #[error("database operation failed: {0}")]
    Database(#[from] sqlx::Error),
    /// A stored value no longer satisfies its domain type (for example a
    /// corrupted or hand-edited row). Reported, never trusted or panicked on.
    #[error("invalid value in the database: {0}")]
    InvalidStoredValue(#[from] FieldError),
    #[error("could not encode a query parameter: {0}")]
    EncodeParameter(#[from] serde_json::Error),
}

/// An unusable `[persistence]` setting. Reported at startup, before any
/// connection is attempted.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PersistenceConfigError {
    #[error("configuration setting `persistence.database_path` must not be empty")]
    EmptyDatabasePath,
    #[error("persistence.max_connections must be at least 1")]
    ZeroMaxConnections,
    #[error("persistence.acquire_timeout_ms must be greater than 0")]
    ZeroAcquireTimeout,
    #[error("persistence.busy_timeout_ms must be at most {max} (SQLite's limit), got {value}")]
    BusyTimeoutTooLarge { value: u64, max: u64 },
}
