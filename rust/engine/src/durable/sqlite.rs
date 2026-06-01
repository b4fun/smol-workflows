//! SQLite durable workflow store infrastructure.
//!
//! This module owns database opening, pragmatic SQLite setup, and schema
//! migrations. The migration shape follows the Absurd SQLite extension pattern:
//! numbered SQL files are embedded at compile time, applied inside one immediate
//! transaction, and recorded in a migrations table.

use anyhow::{anyhow, bail, Context};
use rusqlite::{params, Connection};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

mod embedded_migrations {
    include!(concat!(env!("OUT_DIR"), "/smol_workflow_migrations.rs"));
}

const MIGRATIONS_TABLE_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS sw_migrations (
    id INTEGER PRIMARY KEY,
    introduced_version TEXT NOT NULL,
    applied_time INTEGER NOT NULL
)
"#;

/// One applied durable schema migration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationRecord {
    pub id: i64,
    pub introduced_version: String,
    pub applied_time: i64,
}

/// SQLite-backed durable workflow store.
pub struct SqliteDurableStore {
    connection: Connection,
}

impl SqliteDurableStore {
    /// Open a durable store at `path` and apply connection pragmas.
    pub fn open(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let connection = Connection::open(path.as_ref()).with_context(|| {
            format!(
                "failed to open durable SQLite database {}",
                path.as_ref().display()
            )
        })?;
        configure_connection(&connection)?;
        Ok(Self { connection })
    }

    /// Create an in-memory durable store. Useful for tests.
    pub fn in_memory() -> anyhow::Result<Self> {
        let connection = Connection::open_in_memory()
            .context("failed to open in-memory durable SQLite database")?;
        configure_connection(&connection)?;
        Ok(Self { connection })
    }

    /// Borrow the underlying SQLite connection.
    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    /// Mutably borrow the underlying SQLite connection.
    pub fn connection_mut(&mut self) -> &mut Connection {
        &mut self.connection
    }

    /// Initialize the durable schema by applying all available migrations.
    pub fn init(&mut self) -> anyhow::Result<usize> {
        self.apply_migrations(None)
    }

    /// Apply migrations up to `target_version`, or all available migrations when
    /// `target_version` is `None`.
    ///
    /// Returns the number of migrations applied in this call.
    pub fn apply_migrations(&mut self, target_version: Option<i64>) -> anyhow::Result<usize> {
        configure_connection(&self.connection)?;
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .context("failed to begin durable migration transaction")?;

        let result = (|| -> anyhow::Result<usize> {
            ensure_migrations_table(&tx)?;

            let applied = applied_migration_ids(&tx)?;
            let max_applied = applied.iter().copied().max().unwrap_or(0);
            let max_known = embedded_migrations::MIGRATIONS
                .last()
                .map(|migration| migration.id)
                .unwrap_or(0);
            let target = target_version.unwrap_or(max_known);

            if target < max_applied {
                bail!(
                    "target migration version {target} is older than applied version {max_applied}"
                );
            }
            if target > max_known {
                bail!(
                    "target migration version {target} is newer than available version {max_known}"
                );
            }

            let mut applied_count = 0usize;
            for migration in embedded_migrations::MIGRATIONS
                .iter()
                .filter(|migration| migration.id <= target)
            {
                if applied.contains(&migration.id) {
                    continue;
                }
                tx.execute_batch(migration.sql).with_context(|| {
                    format!("failed to apply durable migration {}", migration.id)
                })?;
                tx.execute(
                    r#"
                    INSERT INTO sw_migrations (
                        id,
                        introduced_version,
                        applied_time
                    )
                    VALUES (?1, ?2, ?3)
                    "#,
                    params![migration.id, migration.introduced_version, now_ms()],
                )
                .with_context(|| format!("failed to record durable migration {}", migration.id))?;
                applied_count += 1;
            }
            Ok(applied_count)
        })();

        match result {
            Ok(applied_count) => {
                tx.commit()
                    .context("failed to commit durable migration transaction")?;
                Ok(applied_count)
            }
            Err(error) => {
                let _ = tx.rollback();
                Err(error)
            }
        }
    }

    /// Return applied migrations in ascending id order.
    pub fn migration_records(&self) -> anyhow::Result<Vec<MigrationRecord>> {
        ensure_migrations_table(&self.connection)?;
        let mut statement = self
            .connection
            .prepare(
                r#"
                SELECT id, introduced_version, applied_time
                FROM sw_migrations
                ORDER BY id
                "#,
            )
            .context("failed to prepare durable migration records query")?;
        let rows = statement
            .query_map([], |row| {
                Ok(MigrationRecord {
                    id: row.get(0)?,
                    introduced_version: row.get(1)?,
                    applied_time: row.get(2)?,
                })
            })
            .context("failed to query durable migration records")?;

        let mut records = Vec::new();
        for row in rows {
            records.push(row.context("failed to read durable migration record")?);
        }
        Ok(records)
    }

    /// Return the latest applied migration id, or `0` when none are applied.
    pub fn current_schema_version(&self) -> anyhow::Result<i64> {
        ensure_migrations_table(&self.connection)?;
        self.connection
            .query_row(
                r#"
                SELECT COALESCE(MAX(id), 0)
                FROM sw_migrations
                "#,
                [],
                |row| row.get(0),
            )
            .context("failed to read durable schema version")
    }
}

fn configure_connection(connection: &Connection) -> anyhow::Result<()> {
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .context("failed to enable SQLite foreign_keys")?;
    connection
        .busy_timeout(std::time::Duration::from_millis(5_000))
        .context("failed to configure SQLite busy_timeout")?;

    let journal_mode: String = connection
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .context("failed to read SQLite journal_mode")?;
    if !journal_mode.eq_ignore_ascii_case("memory") {
        let mode: String = connection
            .pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))
            .context("failed to enable SQLite WAL journal_mode")?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err(anyhow!("expected SQLite journal_mode WAL, found {mode}"));
        }
    }

    Ok(())
}

fn ensure_migrations_table(connection: &Connection) -> anyhow::Result<()> {
    connection
        .execute_batch(MIGRATIONS_TABLE_SQL)
        .context("failed to ensure durable migrations table")
}

fn applied_migration_ids(
    connection: &Connection,
) -> anyhow::Result<std::collections::HashSet<i64>> {
    let mut statement = connection
        .prepare(
            r#"
            SELECT id
            FROM sw_migrations
            ORDER BY id
            "#,
        )
        .context("failed to prepare applied durable migrations query")?;
    let rows = statement
        .query_map([], |row| row.get::<_, i64>(0))
        .context("failed to query applied durable migrations")?;
    let mut ids = std::collections::HashSet::new();
    for row in rows {
        ids.insert(row.context("failed to read applied durable migration id")?);
    }
    Ok(ids)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initializes_schema_and_records_migration() {
        let mut store = SqliteDurableStore::in_memory().expect("store should open");
        let applied = store.init().expect("migrations should apply");
        assert_eq!(applied, embedded_migrations::MIGRATIONS.len());
        assert_eq!(store.current_schema_version().unwrap(), 1);

        let records = store.migration_records().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, 1);
        assert_eq!(records[0].introduced_version, env!("CARGO_PKG_VERSION"));

        let table_count: i64 = store
            .connection()
            .query_row(
                r#"
                SELECT COUNT(*)
                FROM sqlite_master
                WHERE type = 'table'
                  AND name IN (
                      'sw_workflow_tasks',
                      'sw_workflow_runs',
                      'sw_workflow_steps',
                      'sw_budget_ledger'
                  )
                "#,
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(table_count, 4);
    }

    #[test]
    fn migrations_are_idempotent() {
        let mut store = SqliteDurableStore::in_memory().expect("store should open");
        assert_eq!(store.init().unwrap(), 1);
        assert_eq!(store.init().unwrap(), 0);
        assert_eq!(store.migration_records().unwrap().len(), 1);
    }

    #[test]
    fn rejects_target_older_than_applied_version() {
        let mut store = SqliteDurableStore::in_memory().expect("store should open");
        store.init().unwrap();
        let error = store.apply_migrations(Some(0)).unwrap_err();
        assert!(
            error.to_string().contains("older than applied"),
            "unexpected error: {error:#}"
        );
    }
}
