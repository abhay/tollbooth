use super::LibsqlStore;
use crate::error::StoreError;
use crate::types::WebhookEntry;

impl LibsqlStore {
    pub async fn enqueue_webhook(
        &self,
        event_type: &str,
        payload: &serde_json::Value,
    ) -> Result<(), StoreError> {
        let now = crate::types::now_secs();
        let payload_str = serde_json::to_string(payload)
            .map_err(|e| StoreError::Database(format!("serialize webhook payload: {e}")))?;

        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO webhook_queue (event_type, payload, created_at) VALUES (?1, ?2, ?3)",
            libsql::params![event_type, payload_str, now],
        )
        .await?;
        Ok(())
    }

    pub async fn dequeue_webhooks(&self, limit: usize) -> Result<Vec<WebhookEntry>, StoreError> {
        let conn = self.conn()?;
        let mut rows = conn
            .query(
                "SELECT id, event_type, payload, created_at, attempts FROM webhook_queue WHERE delivered = 0 AND attempts < 3 ORDER BY created_at LIMIT ?1",
                [limit as i64],
            )
            .await?;

        let mut entries = Vec::new();
        while let Some(row) = rows.next().await? {
            let payload_str: String = row.get(2)?;
            let payload: serde_json::Value = serde_json::from_str(&payload_str)
                .map_err(|e| StoreError::Database(format!("corrupt webhook payload: {e}")))?;
            entries.push(WebhookEntry {
                id: row.get(0)?,
                event_type: row.get(1)?,
                payload,
                created_at: row.get(3)?,
                attempts: row.get::<i64>(4)? as i32,
            });
        }
        Ok(entries)
    }

    pub async fn mark_webhook_delivered(&self, id: i64) -> Result<(), StoreError> {
        let conn = self.conn()?;
        conn.execute("UPDATE webhook_queue SET delivered = 1 WHERE id = ?1", [id])
            .await?;
        Ok(())
    }

    pub async fn mark_webhook_failed(&self, id: i64) -> Result<(), StoreError> {
        let now = crate::types::now_secs();
        let conn = self.conn()?;
        conn.execute(
            "UPDATE webhook_queue SET attempts = attempts + 1, last_attempt_at = ?2 WHERE id = ?1",
            libsql::params![id, now],
        )
        .await?;
        Ok(())
    }
}
