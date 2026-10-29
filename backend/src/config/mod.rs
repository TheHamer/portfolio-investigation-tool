//! Runtime configuration: a required TOML file, overlaid by environment
//! variables, deserialised into typed fields, then validated.
//!
//! The application settings (dataset paths, bind address) have no defaults:
//! each must be present in the merged configuration (file plus environment),
//! or startup fails. The `[persistence]` section is required too, and so is its
//! `database_path`; its four operational settings have individual defaults,
//! defined in `persistence::config`. The `[api]` section is optional, and so is
//! each of its keys, with defaults defined in `api::config`. Relative paths are
//! resolved against the process's working directory.

mod error;
mod strict;

pub use error::ConfigError;
pub(crate) use strict::{strict_bool, strict_uint};

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use ::config as config_rs;
use serde::Deserialize;

use crate::api::{ApiConfig, RawApiConfig};
use crate::persistence::{PersistenceConfig, RawPersistenceConfig};

/// Looked up relative to the working directory when no path is given.
const DEFAULT_CONFIG_FILE: &str = "config.toml";

/// Prefix for environment overrides: `INTROPIC_BIND_ADDR` overrides
/// `bind_addr`, `INTROPIC_MANIFEST_PATH` overrides `manifest_path`, and so on.
pub const ENV_PREFIX: &str = "INTROPIC";

/// Separates nested keys in environment variable names:
/// `INTROPIC_PERSISTENCE__DATABASE_PATH` overrides `persistence.database_path`.
/// Top-level keys contain single underscores only, so they are unaffected.
const ENV_NESTED_SEPARATOR: &str = "__";

/// The settings exactly as written in the file and environment. Private: the
/// only way out of it is `Config::from_raw`, which validates.
#[derive(Debug, Deserialize)]
// Unknown keys (a typo in the file, or an unrecognised INTROPIC_* variable)
// are errors rather than silently ignored settings.
#[serde(deny_unknown_fields)]
struct RawConfig {
    manifest_path: PathBuf,
    reference_data_dir: PathBuf,
    bind_addr: SocketAddr,
    static_dir: Option<PathBuf>,
    /// Required section; within it only `database_path` is required.
    persistence: RawPersistenceConfig,
    /// Optional section; absent means every key takes its default.
    #[serde(default)]
    api: RawApiConfig,
}

/// Validated, read-only configuration.
///
/// Fields are private and there is no public constructor or `Deserialize`
/// impl: a `Config` can only come from [`Config::load`], so every instance
/// has passed [`Config::validate`].
///
/// Usage:
///
/// ```no_run
/// use std::path::Path;
/// use portfolio_investigation::config::Config;
///
/// let config = Config::load(Some(Path::new("config.toml")), None)?;
/// println!("listening on {}", config.bind_addr());
/// # Ok::<(), portfolio_investigation::config::ConfigError>(())
/// ```
#[derive(Debug, Clone)]
pub struct Config {
    /// The dataset's `manifest.json`.
    manifest_path: PathBuf,
    /// Directory holding `portfolios.csv`, `instruments.csv`, `prices.csv`
    /// and `fx_rates.csv`.
    reference_data_dir: PathBuf,
    bind_addr: SocketAddr,
    /// Built frontend (`frontend/dist`). When set, Axum serves the React app
    /// from it (production, Docker) and it must contain `index.html`. When
    /// absent the server is API only, for local development with the Vite
    /// server.
    static_dir: Option<PathBuf>,
    /// Database file and connection-pool settings (validated).
    persistence: PersistenceConfig,
    /// HTTP API settings (validated).
    api: ApiConfig,
}

impl Config {
    /// Loads the configuration file, applies `INTROPIC_*` environment
    /// overrides and validates the result. `None` selects `config.toml` in the
    /// working directory. `environment` replaces the process environment when
    /// given (`None` in production), so tests can exercise the real override
    /// mechanism without mutating global state.
    pub fn load(
        path: Option<&Path>,
        environment: Option<HashMap<String, String>>,
    ) -> Result<Self, ConfigError> {
        let path = path.unwrap_or_else(|| Path::new(DEFAULT_CONFIG_FILE));
        tracing::info!(config_file = %path.display(), "loading configuration");

        let merged = config_rs::Config::builder()
            .add_source(
                config_rs::File::from(path)
                    .format(config_rs::FileFormat::Toml)
                    .required(true),
            )
            // Environment values arrive as strings and are converted to the
            // field types (integers, booleans) during deserialisation.
            .add_source(
                config_rs::Environment::with_prefix(ENV_PREFIX)
                    .prefix_separator("_")
                    .separator(ENV_NESTED_SEPARATOR)
                    .source(environment),
            )
            .build()
            .map_err(|source| ConfigError::Read {
                path: path.to_path_buf(),
                source,
            })?;

        let raw: RawConfig = merged
            .try_deserialize()
            .map_err(|source| ConfigError::Invalid {
                path: path.to_path_buf(),
                source,
            })?;
        Self::from_raw(raw)
    }

    /// The only constructor: validates each section while moving the fields
    /// across, then checks the remaining invariants.
    fn from_raw(raw: RawConfig) -> Result<Self, ConfigError> {
        let RawConfig {
            manifest_path,
            reference_data_dir,
            bind_addr,
            static_dir,
            persistence,
            api,
        } = raw;

        let config = Self {
            manifest_path,
            reference_data_dir,
            bind_addr,
            static_dir,
            persistence: PersistenceConfig::from_raw(persistence)?,
            api: ApiConfig::from_raw(api)?,
        };
        config.validate()?;
        Ok(config)
    }

    /// Invariants that deserialisation cannot express. Types already
    /// guarantee presence and syntax (`SocketAddr`); this checks emptiness and
    /// that a configured frontend directory holds a build.
    ///
    /// The manifest and reference files are not opened here: their loaders
    /// report a missing or invalid file with its path. Persistence and API
    /// settings are validated when `PersistenceConfig` and `ApiConfig` are
    /// built.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.manifest_path.as_os_str().is_empty() {
            return Err(ConfigError::EmptyPath("manifest_path"));
        }
        if self.reference_data_dir.as_os_str().is_empty() {
            return Err(ConfigError::EmptyPath("reference_data_dir"));
        }

        if let Some(dir) = self.static_dir.as_deref() {
            if dir.as_os_str().is_empty() {
                return Err(ConfigError::EmptyPath("static_dir"));
            }
            if !dir.join("index.html").is_file() {
                return Err(ConfigError::MissingFrontendAssets {
                    path: dir.to_path_buf(),
                });
            }
        }

        Ok(())
    }

    pub fn manifest_path(&self) -> &Path {
        &self.manifest_path
    }

    pub fn reference_data_dir(&self) -> &Path {
        &self.reference_data_dir
    }

    pub fn bind_addr(&self) -> SocketAddr {
        self.bind_addr
    }

    /// `Some` when the frontend is served, `None` in API-only mode.
    pub fn static_dir(&self) -> Option<&Path> {
        self.static_dir.as_deref()
    }

    /// Database file and connection-pool settings, for `persistence::connect`.
    pub fn persistence(&self) -> &PersistenceConfig {
        &self.persistence
    }

    /// HTTP API settings, for the router and the request handlers.
    pub fn api(&self) -> &ApiConfig {
        &self.api
    }
}

#[cfg(test)]
mod tests {
    //! Loading, merging and overrides. Each section's own defaults and
    //! validation rules are tested in `persistence::config` and `api::config`.

    use std::time::Duration;

    use super::*;
    use crate::api::ApiConfigError;
    use crate::persistence::PersistenceConfigError;
    use tempfile::TempDir;

    /// A complete configuration. The required database path is written as a
    /// TOML dotted key, which defines the `[persistence]` table without a
    /// header, so tests can append top-level keys or more `persistence.*` keys.
    const VALID: &str = r#"
        manifest_path = "../data/manifest.json"
        reference_data_dir = "../data/reference"
        bind_addr = "127.0.0.1:8080"
        persistence.database_path = "var/portfolio.db"
    "#;

    /// A directory that satisfies the `static_dir` check.
    fn built_frontend(dir: &TempDir) -> PathBuf {
        let static_dir = dir.path().join("dist");
        std::fs::create_dir_all(&static_dir).unwrap();
        std::fs::write(static_dir.join("index.html"), "<!doctype html>").unwrap();
        static_dir
    }

    fn env(vars: &[(&str, &str)]) -> Option<HashMap<String, String>> {
        Some(
            vars.iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    }

    fn load(contents: &str, vars: &[(&str, &str)]) -> Result<Config, ConfigError> {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, contents).unwrap();
        Config::load(Some(&path), env(vars))
    }

    #[test]
    fn valid_file_loads_with_section_defaults() {
        let config = load(VALID, &[]).unwrap();

        assert_eq!(config.manifest_path(), Path::new("../data/manifest.json"));
        assert_eq!(config.reference_data_dir(), Path::new("../data/reference"));
        assert_eq!(config.bind_addr(), "127.0.0.1:8080".parse().unwrap());
        assert_eq!(config.static_dir(), None);
        assert_eq!(
            config.persistence().database_path(),
            Path::new("var/portfolio.db")
        );
        // Optional settings, and the whole [api] section, take their defaults.
        assert_eq!(config.persistence().max_connections(), 5);
        assert_eq!(config.api().max_upload_bytes(), 10_485_760);
    }

    #[test]
    fn unreadable_file_is_an_error_naming_the_path() {
        let dir = TempDir::new().unwrap();
        let error = Config::load(Some(&dir.path().join("absent.toml")), env(&[])).unwrap_err();
        assert!(matches!(error, ConfigError::Read { .. }), "{error}");
        assert!(error.to_string().contains("absent.toml"), "{error}");

        let error = load("manifest_path = \"unterminated", &[]).unwrap_err();
        assert!(matches!(error, ConfigError::Read { .. }), "{error}");
    }

    #[test]
    fn missing_required_settings_are_named() {
        for (setting, key) in [
            ("manifest_path = \"../data/manifest.json\"", "manifest_path"),
            (
                "reference_data_dir = \"../data/reference\"",
                "reference_data_dir",
            ),
            ("bind_addr = \"127.0.0.1:8080\"", "bind_addr"),
            (
                "persistence.database_path = \"var/portfolio.db\"",
                "persistence",
            ),
        ] {
            let error = load(&VALID.replace(setting, ""), &[]).unwrap_err();
            assert!(matches!(error, ConfigError::Invalid { .. }), "{error}");
            assert!(error.to_string().contains(key), "{key}: {error}");
        }

        // The section is present but its required key is not.
        let contents = VALID.replace(
            "database_path = \"var/portfolio.db\"",
            "max_connections = 5",
        );
        let error = load(&contents, &[]).unwrap_err();
        assert!(error.to_string().contains("database_path"), "{error}");
    }

    #[test]
    fn required_settings_may_come_from_the_environment() {
        let without = VALID
            .replace("bind_addr = \"127.0.0.1:8080\"", "")
            .replace("persistence.database_path = \"var/portfolio.db\"", "");
        let config = load(
            &without,
            &[
                ("INTROPIC_BIND_ADDR", "0.0.0.0:9000"),
                ("INTROPIC_PERSISTENCE__DATABASE_PATH", "/tmp/from-env.db"),
            ],
        )
        .unwrap();

        assert_eq!(config.bind_addr(), "0.0.0.0:9000".parse().unwrap());
        assert_eq!(
            config.persistence().database_path(),
            Path::new("/tmp/from-env.db")
        );
    }

    #[test]
    fn environment_overrides_only_the_matching_setting() {
        let dir = TempDir::new().unwrap();
        let static_dir = built_frontend(&dir);

        let config = load(
            VALID,
            &[
                ("INTROPIC_MANIFEST_PATH", "/srv/data/manifest.json"),
                ("INTROPIC_REFERENCE_DATA_DIR", "/srv/data/reference"),
                ("INTROPIC_STATIC_DIR", static_dir.to_str().unwrap()),
                ("INTROPIC_BIND_ADDR", "0.0.0.0:8081"),
                ("UNRELATED_BIND_ADDR", "1.2.3.4:1"),
            ],
        )
        .unwrap();

        assert_eq!(config.manifest_path(), Path::new("/srv/data/manifest.json"));
        assert_eq!(
            config.reference_data_dir(),
            Path::new("/srv/data/reference")
        );
        assert_eq!(config.static_dir(), Some(static_dir.as_path()));
        assert_eq!(config.bind_addr(), "0.0.0.0:8081".parse().unwrap());
        assert_eq!(
            config.persistence().database_path(),
            Path::new("var/portfolio.db")
        );
    }

    #[test]
    fn nested_overrides_apply_key_by_key() {
        let contents = format!("{VALID}\npersistence.acquire_timeout_ms = 900\n");
        let config = load(
            &contents,
            &[
                ("INTROPIC_PERSISTENCE__MAX_CONNECTIONS", "9"),
                ("INTROPIC_PERSISTENCE__CREATE_IF_MISSING", "false"),
                ("INTROPIC_API__MAX_UPLOAD_BYTES", "512"),
            ],
        )
        .unwrap();

        let persistence = config.persistence();
        assert_eq!(persistence.max_connections(), 9);
        assert!(!persistence.create_if_missing());
        // The rest come from the file and from the defaults.
        assert_eq!(persistence.acquire_timeout(), Duration::from_millis(900));
        assert_eq!(persistence.busy_timeout(), Duration::from_millis(10_000));
        assert_eq!(persistence.database_path(), Path::new("var/portfolio.db"));
        assert_eq!(config.api().max_upload_bytes(), 512);
    }

    #[test]
    fn unknown_settings_are_rejected() {
        for contents in [
            format!("{VALID}\nbind_adress = \"x\""),
            format!("{VALID}\npersistence.pool_size = 3"),
            format!("{VALID}\napi.max_upload = 1024"),
        ] {
            let error = load(&contents, &[]).unwrap_err();
            assert!(matches!(error, ConfigError::Invalid { .. }), "{error}");
        }

        for key in [
            "INTROPIC_BIND_ADRESS",
            "INTROPIC_DATABASE_PATH",
            "INTROPIC_PERSISTENCE__POOL_SIZE",
        ] {
            assert!(load(VALID, &[(key, "1")]).is_err(), "{key}");
        }
    }

    #[test]
    fn values_of_the_wrong_type_are_rejected_not_converted() {
        for setting in [
            "persistence.max_connections = 5.5",
            "persistence.max_connections = -1",
            "persistence.busy_timeout_ms = true",
            "persistence.acquire_timeout_ms = 1e3",
            "persistence.create_if_missing = 1",
            "persistence.create_if_missing = \"yes\"",
            "api.max_upload_bytes = \"lots\"",
        ] {
            let error = load(&format!("{VALID}\n{setting}\n"), &[]).unwrap_err();
            assert!(
                matches!(error, ConfigError::Invalid { .. }),
                "{setting}: {error}"
            );
        }

        let error = load(&VALID.replace("127.0.0.1:8080", "localhost"), &[]).unwrap_err();
        assert!(matches!(error, ConfigError::Invalid { .. }), "{error}");

        for (key, value) in [
            ("INTROPIC_BIND_ADDR", "not-an-address"),
            ("INTROPIC_PERSISTENCE__MAX_CONNECTIONS", "5.5"),
            ("INTROPIC_PERSISTENCE__CREATE_IF_MISSING", "yes"),
            ("INTROPIC_API__MAX_UPLOAD_BYTES", "-1"),
        ] {
            let error = load(VALID, &[(key, value)]).unwrap_err();
            assert!(
                matches!(error, ConfigError::Invalid { .. }),
                "{key}={value}: {error}"
            );
        }
    }

    #[test]
    fn each_section_is_validated_during_load() {
        let error = load(VALID, &[("INTROPIC_MANIFEST_PATH", "")]).unwrap_err();
        assert!(
            matches!(error, ConfigError::EmptyPath("manifest_path")),
            "{error}"
        );

        let error = load(&format!("{VALID}\npersistence.max_connections = 0\n"), &[]).unwrap_err();
        assert!(
            matches!(
                error,
                ConfigError::Persistence(PersistenceConfigError::ZeroMaxConnections)
            ),
            "{error}"
        );

        let error = load(VALID, &[("INTROPIC_API__MAX_UPLOAD_BYTES", "0")]).unwrap_err();
        assert!(
            matches!(error, ConfigError::Api(ApiConfigError::ZeroMaxUploadBytes)),
            "{error}"
        );
    }

    #[test]
    fn a_static_dir_requires_built_assets() {
        let dir = TempDir::new().unwrap();
        let static_dir = dir.path().join("dist");
        std::fs::create_dir_all(&static_dir).unwrap();
        let contents = format!("static_dir = {:?}\n{VALID}", static_dir.to_str().unwrap());

        let error = load(&contents, &[]).unwrap_err();
        assert!(
            matches!(error, ConfigError::MissingFrontendAssets { .. }),
            "{error}"
        );

        std::fs::write(static_dir.join("index.html"), "<!doctype html>").unwrap();
        let config = load(&contents, &[]).unwrap();
        assert_eq!(config.static_dir(), Some(static_dir.as_path()));
    }

    #[test]
    fn committed_configuration_files_are_valid() {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let local = Config::load(Some(&manifest_dir.join("config.toml")), env(&[]))
            .expect("backend/config.toml loads");

        assert_eq!(local.static_dir(), None);
        assert_eq!(local.api().max_upload_bytes(), 10_485_760);

        // The container file serves the frontend from /app/static, which only
        // exists in the image; getting this far proves it parses completely.
        let docker =
            Config::load(Some(&manifest_dir.join("config.docker.toml")), env(&[])).unwrap_err();

        match docker {
            ConfigError::MissingFrontendAssets { path } => {
                assert_eq!(path, PathBuf::from("/app/static"))
            }
            other => panic!("unexpected error: {other}"),
        }
    }
}
