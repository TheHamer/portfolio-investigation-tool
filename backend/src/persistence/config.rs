//! Database connection settings: the required `[persistence]` section of the
//! configuration file.
//!
//! `database_path` is required. The four operational settings are optional,
//! each with its own default, applied independently of the others.
//! Integrity and durability policies (foreign keys, WAL, synchronous FULL)
//! are deliberately not configurable; see `connection.rs`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

use super::error::PersistenceConfigError;
use crate::config::{strict_bool, strict_uint};

// The defaults, in one place. Each is used only when its key is absent; an
// explicit value (including 0 or false) is always kept.
fn default_max_connections() -> u32 {
    5
}

fn default_busy_timeout_ms() -> u64 {
    10_000
}

fn default_acquire_timeout_ms() -> u64 {
    30_000
}

fn default_create_if_missing() -> bool {
    true
}

/// The `[persistence]` section as written. Only `PersistenceConfig::from_raw`
/// turns it into a usable configuration.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawPersistenceConfig {
    database_path: PathBuf,
    #[serde(default = "default_max_connections", deserialize_with = "strict_uint")]
    max_connections: u32,
    #[serde(default = "default_busy_timeout_ms", deserialize_with = "strict_uint")]
    busy_timeout_ms: u64,
    #[serde(
        default = "default_acquire_timeout_ms",
        deserialize_with = "strict_uint"
    )]
    acquire_timeout_ms: u64,
    #[serde(
        default = "default_create_if_missing",
        deserialize_with = "strict_bool"
    )]
    create_if_missing: bool,
}

/// Validated database connection settings. Built only from the configuration
/// file (through `Config::load`), so the values are always usable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistenceConfig {
    database_path: PathBuf,
    max_connections: u32,
    busy_timeout_ms: u64,
    acquire_timeout_ms: u64,
    create_if_missing: bool,
}

impl PersistenceConfig {
    /// SQLite stores the busy timeout as a C `int` of milliseconds; SQLx
    /// panics when the value does not fit, so larger values are rejected here.
    pub const MAX_BUSY_TIMEOUT_MS: u64 = i32::MAX as u64;

    pub(crate) fn from_raw(raw: RawPersistenceConfig) -> Result<Self, PersistenceConfigError> {
        if raw.database_path.as_os_str().is_empty() {
            return Err(PersistenceConfigError::EmptyDatabasePath);
        }
        if raw.max_connections == 0 {
            return Err(PersistenceConfigError::ZeroMaxConnections);
        }

        // Zero is allowed: SQLite then returns SQLITE_BUSY immediately instead
        // of waiting for a lock (a deliberate fail-fast setting).
        if raw.busy_timeout_ms > Self::MAX_BUSY_TIMEOUT_MS {
            return Err(PersistenceConfigError::BusyTimeoutTooLarge {
                value: raw.busy_timeout_ms,
                max: Self::MAX_BUSY_TIMEOUT_MS,
            });
        }

        // SQLx applies this timeout to every acquire, so zero would make every
        // database operation fail.
        if raw.acquire_timeout_ms == 0 {
            return Err(PersistenceConfigError::ZeroAcquireTimeout);
        }

        Ok(Self {
            database_path: raw.database_path,
            max_connections: raw.max_connections,
            busy_timeout_ms: raw.busy_timeout_ms,
            acquire_timeout_ms: raw.acquire_timeout_ms,
            create_if_missing: raw.create_if_missing,
        })
    }

    /// The SQLite database file.
    pub fn database_path(&self) -> &Path {
        &self.database_path
    }

    /// The pool's maximum number of open connections.
    pub fn max_connections(&self) -> u32 {
        self.max_connections
    }

    /// How long SQLite waits for a lock held by another connection before
    /// returning SQLITE_BUSY (`sqlite3_busy_timeout`).
    pub fn busy_timeout(&self) -> Duration {
        Duration::from_millis(self.busy_timeout_ms)
    }

    /// How long a caller waits for a free connection from the SQLx pool.
    pub fn acquire_timeout(&self) -> Duration {
        Duration::from_millis(self.acquire_timeout_ms)
    }

    /// Whether a missing database file (and its directory) may be created.
    pub fn create_if_missing(&self) -> bool {
        self.create_if_missing
    }
}

#[cfg(test)]
impl PersistenceConfig {
    /// Default settings for `database_path`, through the normal validation,
    /// for crate tests that only need a working pool.
    pub(crate) fn for_tests(database_path: PathBuf) -> Self {
        Self::from_raw(RawPersistenceConfig {
            database_path,
            max_connections: default_max_connections(),
            busy_timeout_ms: default_busy_timeout_ms(),
            acquire_timeout_ms: default_acquire_timeout_ms(),
            create_if_missing: default_create_if_missing(),
        })
        .expect("defaults are valid")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deserialises a section the way the loader does (via serde), then
    /// validates it.
    fn parse(json: serde_json::Value) -> Result<PersistenceConfig, String> {
        let raw: RawPersistenceConfig = serde_json::from_value(json).map_err(|e| e.to_string())?;
        PersistenceConfig::from_raw(raw).map_err(|e| e.to_string())
    }

    #[test]
    fn each_optional_setting_defaults_independently() {
        let only_path = parse(serde_json::json!({ "database_path": "db.sqlite" })).unwrap();
        assert_eq!(only_path.database_path(), Path::new("db.sqlite"));
        assert_eq!(only_path.max_connections(), 5);
        assert_eq!(only_path.busy_timeout(), Duration::from_secs(10));
        assert_eq!(only_path.acquire_timeout(), Duration::from_secs(30));
        assert!(only_path.create_if_missing());

        let partial = parse(serde_json::json!({
            "database_path": "db.sqlite",
            "max_connections": 12,
            "create_if_missing": false,
        }))
        .unwrap();

        assert_eq!(partial.max_connections(), 12);
        assert!(!partial.create_if_missing());
        assert_eq!(partial.busy_timeout(), Duration::from_secs(10));
        assert_eq!(partial.acquire_timeout(), Duration::from_secs(30));
    }

    #[test]
    fn database_path_is_required_and_non_empty() {
        let missing = parse(serde_json::json!({ "max_connections": 5 })).unwrap_err();
        assert!(missing.contains("database_path"), "{missing}");

        let empty = parse(serde_json::json!({ "database_path": "" })).unwrap_err();
        assert_eq!(empty, PersistenceConfigError::EmptyDatabasePath.to_string());
    }

    #[test]
    fn validation_rules() {
        let with = |max: u64, busy: u64, acquire: u64| {
            parse(serde_json::json!({
                "database_path": "db.sqlite",
                "max_connections": max,
                "busy_timeout_ms": busy,
                "acquire_timeout_ms": acquire,
            }))
        };

        let max_busy = PersistenceConfig::MAX_BUSY_TIMEOUT_MS;
        assert_eq!(
            with(0, 10_000, 30_000).unwrap_err(),
            PersistenceConfigError::ZeroMaxConnections.to_string()
        );
        assert_eq!(
            with(5, 10_000, 0).unwrap_err(),
            PersistenceConfigError::ZeroAcquireTimeout.to_string()
        );
        assert!(with(5, max_busy + 1, 30_000).is_err());
        // Out of u32 range for max_connections: rejected while deserialising.
        assert!(with(u64::from(u32::MAX) + 1, 10_000, 30_000).is_err());

        // Allowed boundaries: fail-fast busy handling, the largest busy timeout
        // SQLite accepts, a single connection, a 1 ms acquire timeout.
        let config = with(1, 0, 1).unwrap();
        assert_eq!(config.busy_timeout(), Duration::ZERO);
        assert!(with(1, max_busy, 1).is_ok());
    }
}
