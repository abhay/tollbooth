use super::LibsqlStore;
use crate::error::StoreError;
use crate::types::{SessionState, SessionStatus};

const SESSION_COLUMNS: &str = "session_id, bearer_hash, deposit_amount, spent, refund_address, mint, decimals, status, refund_signature, created_at, updated_at";

fn status_str(status: SessionStatus) -> &'static str {
    match status {
        SessionStatus::Active => "active",
        SessionStatus::Closing => "closing",
        SessionStatus::Closed => "closed",
    }
}

impl LibsqlStore {
    pub async fn create_session(&self, session: &SessionState) -> Result<(), StoreError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO sessions (session_id, bearer_hash, deposit_amount, spent, refund_address, mint, decimals, status, refund_signature, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            libsql::params![
                session.session_id.clone(),
                session.bearer_hash.clone(),
                session.deposit_amount as i64,
                session.spent as i64,
                session.refund_address.clone(),
                session.mint.clone(),
                session.decimals as i64,
                status_str(session.status),
                session.refund_signature.clone(),
                session.created_at,
                session.updated_at,
            ],
        )
        .await?;
        Ok(())
    }

    pub async fn get_session(&self, session_id: &str) -> Result<Option<SessionState>, StoreError> {
        let conn = self.conn()?;
        let mut rows = conn
            .query(
                &format!("SELECT {SESSION_COLUMNS} FROM sessions WHERE session_id = ?1"),
                [session_id],
            )
            .await?;

        match rows.next().await? {
            Some(row) => Ok(Some(Self::row_to_session(&row)?)),
            None => Ok(None),
        }
    }

    pub async fn update_session(&self, session: &SessionState) -> Result<(), StoreError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE sessions SET bearer_hash = ?2, deposit_amount = ?3, spent = ?4, refund_address = ?5, status = ?6, refund_signature = ?7, updated_at = ?8 WHERE session_id = ?1",
            libsql::params![
                session.session_id.clone(),
                session.bearer_hash.clone(),
                session.deposit_amount as i64,
                session.spent as i64,
                session.refund_address.clone(),
                status_str(session.status),
                session.refund_signature.clone(),
                session.updated_at,
            ],
        )
        .await?;
        Ok(())
    }

    /// Atomically deduct `cost` from a session's balance, verifying the bearer hash.
    /// Returns the updated session, or None if the session is not active, bearer hash
    /// doesn't match, or there are insufficient funds.
    pub async fn debit_session(
        &self,
        session_id: &str,
        cost: u64,
        now: i64,
        bearer_hash: &str,
    ) -> Result<Option<SessionState>, StoreError> {
        let conn = self.conn()?;
        let mut rows = conn
            .query(
                &format!("UPDATE sessions SET spent = spent + ?1, updated_at = ?2 WHERE session_id = ?3 AND status = 'active' AND bearer_hash = ?4 AND (deposit_amount - spent) >= ?1 RETURNING {SESSION_COLUMNS}"),
                libsql::params![cost as i64, now, session_id, bearer_hash],
            )
            .await?;

        match rows.next().await? {
            Some(row) => Ok(Some(Self::row_to_session(&row)?)),
            None => Ok(None),
        }
    }

    /// Atomically credit a session's deposit.
    /// Returns the updated session, or None if the session is not active.
    pub async fn credit_session(
        &self,
        session_id: &str,
        amount: u64,
        now: i64,
    ) -> Result<Option<SessionState>, StoreError> {
        let conn = self.conn()?;
        let mut rows = conn
            .query(
                &format!("UPDATE sessions SET deposit_amount = deposit_amount + ?1, updated_at = ?2 WHERE session_id = ?3 AND status = 'active' RETURNING {SESSION_COLUMNS}"),
                libsql::params![amount as i64, now, session_id],
            )
            .await?;

        match rows.next().await? {
            Some(row) => Ok(Some(Self::row_to_session(&row)?)),
            None => Ok(None),
        }
    }

    /// Atomically transition a session from active to closing.
    /// Returns the session state, or None if not active.
    pub async fn begin_close_session(
        &self,
        session_id: &str,
        now: i64,
    ) -> Result<Option<SessionState>, StoreError> {
        let conn = self.conn()?;
        let mut rows = conn
            .query(
                &format!("UPDATE sessions SET status = 'closing', updated_at = ?1 WHERE session_id = ?2 AND status = 'active' RETURNING {SESSION_COLUMNS}"),
                libsql::params![now, session_id],
            )
            .await?;

        match rows.next().await? {
            Some(row) => Ok(Some(Self::row_to_session(&row)?)),
            None => Ok(None),
        }
    }

    /// Atomically transition a session from closing to closed, optionally setting refund_signature.
    /// Returns the updated session, or None if the session is not in closing status (race/crash guard).
    pub async fn finalize_close_session(
        &self,
        session_id: &str,
        refund_signature: Option<&str>,
        now: i64,
    ) -> Result<Option<SessionState>, StoreError> {
        let conn = self.conn()?;
        let mut rows = conn
            .query(
                &format!("UPDATE sessions SET status = 'closed', refund_signature = ?1, updated_at = ?2 WHERE session_id = ?3 AND status = 'closing' RETURNING {SESSION_COLUMNS}"),
                libsql::params![refund_signature, now, session_id],
            )
            .await?;

        match rows.next().await? {
            Some(row) => Ok(Some(Self::row_to_session(&row)?)),
            None => Ok(None),
        }
    }

    pub async fn find_closing_sessions(&self) -> Result<Vec<SessionState>, StoreError> {
        let conn = self.conn()?;
        let mut rows = conn
            .query(
                &format!("SELECT {SESSION_COLUMNS} FROM sessions WHERE status = 'closing'"),
                (),
            )
            .await?;

        let mut sessions = Vec::new();
        while let Some(row) = rows.next().await? {
            sessions.push(Self::row_to_session(&row)?);
        }
        Ok(sessions)
    }

    fn row_to_session(row: &libsql::Row) -> Result<SessionState, StoreError> {
        let status_str: String = row.get(7)?;
        let status = match status_str.as_str() {
            "active" => SessionStatus::Active,
            "closing" => SessionStatus::Closing,
            "closed" => SessionStatus::Closed,
            _ => {
                return Err(StoreError::Database(format!(
                    "unknown status: {status_str}"
                )));
            }
        };
        Ok(SessionState {
            session_id: row.get(0)?,
            bearer_hash: row.get(1)?,
            deposit_amount: row.get::<i64>(2)? as u64,
            spent: row.get::<i64>(3)? as u64,
            refund_address: row.get(4)?,
            mint: row.get(5)?,
            decimals: row.get::<i64>(6)? as u8,
            status,
            refund_signature: row.get(8)?,
            created_at: row.get(9)?,
            updated_at: row.get(10)?,
        })
    }

    // ---- Fund management queries ----

    /// Add swept_at column if it doesn't exist. Safe to call on every startup.
    ///
    /// In the fee payer model, the server custodies user deposits and earns revenue
    /// from message charges. When a session closes, the "spent" amount is revenue
    /// that can be swept from the escrow wallet to a separate treasury wallet.
    /// `swept_at` tracks when that sweep happened so we don't double-sweep.
    pub async fn migrate_swept_at(&self) -> Result<(), StoreError> {
        let conn = self.conn()?;
        // Check if column exists via PRAGMA
        let mut rows = conn.query("PRAGMA table_info(sessions)", ()).await?;
        let mut has_swept_at = false;
        while let Some(row) = rows.next().await? {
            let name: String = row.get(1).unwrap_or_default();
            if name == "swept_at" {
                has_swept_at = true;
                break;
            }
        }
        if !has_swept_at {
            conn.execute("ALTER TABLE sessions ADD COLUMN swept_at INTEGER", ())
                .await?;
        }
        Ok(())
    }

    /// Sum of `spent` across all closed sessions that haven't been swept yet.
    /// Returns raw token units (u64).
    ///
    /// In the fee payer model, the server earns the "spent" portion of each session.
    /// This query tells the fund manager how much earned revenue is sitting in the
    /// escrow wallet and ready to be transferred to the treasury.
    pub async fn unswept_revenue(&self) -> Result<u64, StoreError> {
        let conn = self.conn()?;
        let mut rows = conn
            .query(
                "SELECT COALESCE(SUM(spent), 0) FROM sessions WHERE status = 'closed' AND swept_at IS NULL",
                (),
            )
            .await?;

        match rows.next().await? {
            Some(row) => Ok(row.get::<i64>(0).unwrap_or(0) as u64),
            None => Ok(0),
        }
    }

    /// Mark all closed, unswept sessions as swept (bounded to sessions updated before `before_ts`).
    /// Returns the number of sessions marked.
    ///
    /// Called after a successful SPL transfer moves revenue from escrow to treasury.
    /// The `before_ts` bound prevents marking sessions that closed after the sweep
    /// query ran (those will be picked up in the next sweep cycle).
    pub async fn mark_swept(&self, now: i64, before_ts: i64) -> Result<u64, StoreError> {
        let conn = self.conn()?;
        let changed = conn
            .execute(
                "UPDATE sessions SET swept_at = ?1 WHERE status = 'closed' AND swept_at IS NULL AND updated_at <= ?2",
                libsql::params![now, before_ts],
            )
            .await?;
        Ok(changed)
    }

    /// Total liabilities: sum of (deposit_amount - spent) for all active and closing sessions.
    /// Returns raw token units (u64).
    ///
    /// Because the fee payer model custodies user deposits, the server has an obligation
    /// to refund unspent balances. This is the total amount the server must be able to
    /// refund if every active session closed right now. The escrow wallet's USDC balance
    /// must always be >= this number.
    pub async fn total_liabilities(&self) -> Result<u64, StoreError> {
        let conn = self.conn()?;
        let mut rows = conn
            .query(
                "SELECT COALESCE(SUM(deposit_amount - spent), 0) FROM sessions WHERE status IN ('active', 'closing')",
                (),
            )
            .await?;

        match rows.next().await? {
            Some(row) => Ok(row.get::<i64>(0).unwrap_or(0) as u64),
            None => Ok(0),
        }
    }

    /// Count of active and closing sessions. Used by the health endpoint to
    /// report operational load and by monitoring to detect anomalies.
    pub async fn active_session_count(&self) -> Result<u64, StoreError> {
        let conn = self.conn()?;
        let mut rows = conn
            .query(
                "SELECT COUNT(*) FROM sessions WHERE status IN ('active', 'closing')",
                (),
            )
            .await?;

        match rows.next().await? {
            Some(row) => Ok(row.get::<i64>(0).unwrap_or(0) as u64),
            None => Ok(0),
        }
    }

    /// Total deposit amount across active and closing sessions (raw token units).
    /// This is the gross amount users have deposited into sessions that are still
    /// open — a broader view than liabilities (which subtracts what's been spent).
    /// Used by the health endpoint to show the full scope of custodied funds.
    pub async fn total_active_deposits(&self) -> Result<u64, StoreError> {
        let conn = self.conn()?;
        let mut rows = conn
            .query(
                "SELECT COALESCE(SUM(deposit_amount), 0) FROM sessions WHERE status IN ('active', 'closing')",
                (),
            )
            .await?;

        match rows.next().await? {
            Some(row) => Ok(row.get::<i64>(0).unwrap_or(0) as u64),
            None => Ok(0),
        }
    }
}
