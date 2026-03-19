use super::LibsqlStore;
use crate::error::StoreError;
use crate::types::PaymentReceipt;

/// Result of an idempotent consume attempt.
pub enum ConsumeResult {
    /// Signature was freshly consumed. Proceed with this receipt.
    Consumed(PaymentReceipt),
    /// Signature was already consumed. Return the cached receipt.
    AlreadyCached(PaymentReceipt),
}

impl LibsqlStore {
    /// Check if a signature was already consumed and return the cached receipt if so.
    /// Single DB query; replaces the old is_consumed + get_cached_receipt pair.
    pub async fn check_consumed_receipt(
        &self,
        signature: &str,
    ) -> Result<Option<PaymentReceipt>, StoreError> {
        let conn = self.conn()?;
        let mut rows = conn
            .query(
                "SELECT receipt_json FROM consumed_signatures WHERE signature = ?1",
                [signature],
            )
            .await?;

        match rows.next().await? {
            Some(row) => {
                let json: Option<String> = row.get(0)?;
                match json {
                    Some(j) => {
                        let receipt: PaymentReceipt = serde_json::from_str(&j)
                            .map_err(|e| StoreError::Database(format!("corrupt receipt: {e}")))?;
                        Ok(Some(receipt))
                    }
                    // Consumed but no cached receipt (shouldn't happen, but handle gracefully)
                    None => Ok(None),
                }
            }
            // Not consumed
            None => Ok(None),
        }
    }

    /// Idempotent consume: mark signature as consumed with a cached receipt.
    /// - If freshly consumed, returns `ConsumeResult::Consumed(receipt)`.
    /// - If already consumed (UNIQUE conflict), returns the cached receipt.
    /// - If already consumed with no cached receipt, returns `StoreError::Conflict`.
    pub async fn consume_idempotent(
        &self,
        signature: &str,
        protocol: &str,
        amount: Option<&str>,
        payer: Option<&str>,
        receipt: &PaymentReceipt,
    ) -> Result<ConsumeResult, StoreError> {
        let now = crate::types::now_secs();
        let receipt_json = serde_json::to_string(receipt)
            .map_err(|e| StoreError::Database(format!("receipt serialization: {e}")))?;

        let conn = self.conn()?;
        let result = conn
            .execute(
                "INSERT INTO consumed_signatures (signature, protocol, consumed_at, amount, payer, receipt_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                libsql::params![signature, protocol, now, amount, payer, receipt_json],
            )
            .await;

        match result {
            Ok(_) => Ok(ConsumeResult::Consumed(receipt.clone())),
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("UNIQUE") || err_str.contains("constraint") {
                    // Already consumed, try to return cached receipt
                    if let Some(cached) = self.check_consumed_receipt(signature).await? {
                        return Ok(ConsumeResult::AlreadyCached(cached));
                    }
                    Err(StoreError::Conflict(format!(
                        "signature already consumed: {signature}"
                    )))
                } else {
                    Err(StoreError::Database(err_str))
                }
            }
        }
    }
}
