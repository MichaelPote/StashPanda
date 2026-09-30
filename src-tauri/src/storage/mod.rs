//! SQLite database: the commit and ref cache, and remembered UI state (`ui_prefs`).
//!
//! One connection behind a mutex, used only through [`Db::run`], which moves
//! the closure onto a blocking thread so the async runtime never waits on
//! disk. WAL mode lets a page read proceed while a background write runs.

pub mod repo_cache;
pub mod ui_prefs;

use std::path::Path;
use std::sync::{Arc, Mutex};

use rusqlite::Connection;

use crate::error::{GitError, Result};

pub const SCHEMA_VERSION: i64 = 2;
const SCHEMA_SQL: &str = include_str!("schema.sql");
const SCHEMA_V2_SQL: &str = include_str!("schema_v2.sql");

pub struct Db {
    conn: Mutex<Connection>,
}

impl Db {
    pub fn open(path: &Path) -> Result<Arc<Db>> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "auto_vacuum", "INCREMENTAL")?;
        let db = Db {
            conn: Mutex::new(conn),
        };
        db.migrate()?;
        Ok(Arc::new(db))
    }

    /// In-memory database for tests.
    #[cfg(test)]
    pub fn open_in_memory() -> Result<Arc<Db>> {
        let conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let db = Db {
            conn: Mutex::new(conn),
        };
        db.migrate()?;
        Ok(Arc::new(db))
    }

    fn migrate(&self) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| GitError::internal("db mutex poisoned"))?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version < 1 {
            conn.execute_batch(SCHEMA_SQL)?;
            conn.pragma_update(None, "user_version", 1)?;
            tracing::info!("cache database initialised at schema version 1");
        }
        if version < 2 {
            conn.execute_batch(SCHEMA_V2_SQL)?;
            conn.pragma_update(None, "user_version", 2)?;
            tracing::info!("database migrated to schema version 2 (UI state tables)");
        }
        // Future migrations: `if version < 3 { ... }`.
        Ok(())
    }

    /// Run `f` with the connection on a blocking thread.
    pub async fn run<F, T>(self: &Arc<Self>, f: F) -> Result<T>
    where
        F: FnOnce(&mut Connection) -> rusqlite::Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let db = Arc::clone(self);
        tokio::task::spawn_blocking(move || {
            let mut conn = db
                .conn
                .lock()
                .map_err(|_| GitError::internal("db mutex poisoned"))?;
            f(&mut conn).map_err(GitError::from)
        })
        .await
        .map_err(|e| GitError::internal(format!("db task panicked: {e}")))?
    }

    /// Synchronous variant for start-up and tests.
    pub fn run_blocking<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&mut Connection) -> rusqlite::Result<T>,
    {
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| GitError::internal("db mutex poisoned"))?;
        f(&mut conn).map_err(GitError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_applies_and_is_versioned() {
        let db = Db::open_in_memory().unwrap();
        let v: i64 = db
            .run_blocking(|c| c.pragma_query_value(None, "user_version", |r| r.get(0)))
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION);
        let tables: Vec<String> = db
            .run_blocking(|c| {
                let mut stmt = c.prepare(
                    "SELECT name FROM sqlite_master WHERE type='table' ORDER BY name",
                )?;
                let rows = stmt.query_map([], |r| r.get(0))?;
                rows.collect()
            })
            .unwrap();
        assert_eq!(tables, vec!["commits", "refs", "repo_ui_prefs", "repos", "ui_prefs"]);
    }
}
