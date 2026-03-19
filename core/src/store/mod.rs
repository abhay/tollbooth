pub mod metrics;
pub mod sessions;
pub mod signatures;
pub mod webhooks;

use crate::error::StoreError;

pub struct LibsqlStore {
    db: libsql::Database,
}

impl LibsqlStore {
    /// Create a store from a `libsql::Database`.
    /// Each operation gets a fresh connection via `db.connect()`, which creates
    /// a new Hrana stream but reuses the underlying HTTP client. This avoids
    /// stream expiry errors on remote (Hrana) connections.
    pub fn new(db: libsql::Database) -> Self {
        Self { db }
    }

    /// Get a fresh connection. Cheap — reuses the HTTP client, just creates
    /// a new stream. Avoids STREAM_EXPIRED errors on infrequent queries.
    fn conn(&self) -> Result<libsql::Connection, StoreError> {
        Ok(self.db.connect()?)
    }

    pub async fn run_migrations(&self) -> Result<(), StoreError> {
        let sql = include_str!("migrations.sql");
        self.conn()?.execute_batch(sql).await?;
        Ok(())
    }
}
