//! Errors from loading and validating the configuration.

use std::path::PathBuf;

use ::config as config_rs;
use thiserror::Error;

use crate::api::ApiConfigError;
use crate::persistence::PersistenceConfigError;

/// Why the configuration could not be loaded. Reported at startup.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// Missing or unreadable file, or invalid TOML.
    #[error("could not read configuration file {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: config_rs::ConfigError,
    },
    /// A setting is missing, unknown or has the wrong type (for example a
    /// `bind_addr` that is not a socket address).
    #[error("invalid configuration in {path} (or INTROPIC_* overrides): {source}")]
    Invalid {
        path: PathBuf,
        #[source]
        source: config_rs::ConfigError,
    },
    #[error("configuration setting `{0}` must not be empty")]
    EmptyPath(&'static str),
    #[error(
        "`static_dir` {path} has no index.html; build the frontend \
         (`npm run build` in frontend/) or remove `static_dir` for API-only mode"
    )]
    MissingFrontendAssets { path: PathBuf },
    #[error(transparent)]
    Persistence(#[from] PersistenceConfigError),
    #[error(transparent)]
    Api(#[from] ApiConfigError),
}
