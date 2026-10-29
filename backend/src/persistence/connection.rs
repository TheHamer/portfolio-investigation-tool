//! SQLite connection pool and migrations.

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};

use super::{PersistenceConfig, PersistenceError};

/// Opens the configured database file and applies migrations. The file (and
/// its directory) is created only when `config.create_if_missing()` allows it.
///
/// Operational limits come from `config`. Integrity and durability are fixed
/// policy, not configuration: foreign keys are always enforced, the journal is
/// always WAL, and `synchronous` is always FULL.
pub async fn connect(config: &PersistenceConfig) -> Result<SqlitePool, PersistenceError> {
    let path = config.database_path();
    if config.create_if_missing()
        && let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent).map_err(|source| PersistenceError::CreateDirectory {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(config.create_if_missing())
        // SQLite does not enforce foreign keys unless asked, per connection.
        // The option is applied to every connection the pool opens.
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal)
        // Every commit is fsynced: a committed import survives a power loss.
        .synchronous(SqliteSynchronous::Full)
        // A concurrent import waits this long for the write lock instead of
        // failing immediately with SQLITE_BUSY.
        .busy_timeout(config.busy_timeout());

    let pool = SqlitePoolOptions::new()
        .max_connections(config.max_connections())
        // How long a caller waits for a free pooled connection.
        .acquire_timeout(config.acquire_timeout())
        .connect_with(options)
        .await
        .map_err(|source| PersistenceError::Open {
            path: path.to_path_buf(),
            source,
        })?;

    sqlx::migrate!("./migrations").run(&pool).await?;

    Ok(pool)
}

/// A trivial read-only query through the existing pool: whether the database
/// can serve a request right now. Used by the readiness probe.
pub async fn ping(pool: &SqlitePool) -> Result<(), PersistenceError> {
    sqlx::query("SELECT 1").execute(pool).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::Duration;

    use sqlx::Row;
    use tempfile::TempDir;

    use super::*;
    use crate::persistence::RawPersistenceConfig;

    fn config(path: &Path, create_if_missing: bool) -> PersistenceConfig {
        let raw: RawPersistenceConfig = serde_json::from_value(serde_json::json!({
            "database_path": path,
            "max_connections": 3,
            "busy_timeout_ms": 1234,
            "acquire_timeout_ms": 5678,
            "create_if_missing": create_if_missing,
        }))
        .unwrap();
        PersistenceConfig::from_raw(raw).unwrap()
    }

    async fn pragma(pool: &SqlitePool, name: &str) -> String {
        let row = sqlx::query(&format!("PRAGMA {name}"))
            .fetch_one(pool)
            .await
            .unwrap();

        // PRAGMA results are integers or text depending on the pragma.
        row.try_get::<i64, _>(0)
            .map(|v| v.to_string())
            .or_else(|_| row.try_get::<String, _>(0))
            .unwrap()
    }

    #[tokio::test]
    async fn settings_and_fixed_policies_are_applied() {
        let dir = TempDir::new().unwrap();
        let pool = connect(&config(&dir.path().join("nested/db.sqlite"), true))
            .await
            .unwrap();

        // Pool settings, read back from the pool itself.
        assert_eq!(pool.options().get_max_connections(), 3);
        assert_eq!(
            pool.options().get_acquire_timeout(),
            Duration::from_millis(5678)
        );

        // Connection settings, read back from SQLite.
        assert_eq!(pragma(&pool, "busy_timeout").await, "1234");
        assert_eq!(pragma(&pool, "foreign_keys").await, "1");
        assert_eq!(pragma(&pool, "journal_mode").await, "wal");
        assert_eq!(pragma(&pool, "synchronous").await, "2"); // 2 = FULL

        // Migrations ran.
        let applied: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(applied >= 1);
    }

    #[tokio::test]
    async fn missing_database_is_not_created_unless_allowed() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("absent/db.sqlite");

        let error = connect(&config(&path, false)).await.unwrap_err();
        assert!(matches!(error, PersistenceError::Open { .. }), "{error}");
        assert!(!path.exists(), "database file was created");
        assert!(!path.parent().unwrap().exists(), "directory was created");
    }

    #[tokio::test]
    async fn existing_database_opens_without_create_if_missing() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("db.sqlite");
        connect(&config(&path, true)).await.unwrap().close().await;

        let pool = connect(&config(&path, false)).await.unwrap();
        let tables: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE name = 'trades'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(tables, 1);
    }
}
