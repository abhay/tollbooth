use super::LibsqlStore;
use crate::error::StoreError;
use crate::types::MetricPoint;

impl LibsqlStore {
    /// Increment a metric value. Timestamp is floored to the current minute.
    pub async fn increment_metric(&self, metric: &str, value: f64) -> Result<(), StoreError> {
        let now = crate::types::now_secs();
        let ts = now - (now % 60); // floor to minute

        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO metrics_1m (ts, metric, value) VALUES (?1, ?2, ?3) ON CONFLICT(ts, metric) DO UPDATE SET value = value + ?3",
            libsql::params![ts, metric, value],
        )
        .await?;
        Ok(())
    }

    /// Rollup metrics from 1m→1h and 1h→1d in a single transaction.
    /// Crash-safe: uses meta table to track last rollup timestamps.
    pub async fn rollup_metrics(&self) -> Result<(), StoreError> {
        let conn = self.conn()?;
        let now = crate::types::now_secs();

        // 1m → 1h: aggregate rows older than 24 hours
        let cutoff_1h = now - 86400;
        // 1h → 1d: aggregate rows older than 30 days
        let cutoff_1d = now - (30 * 86400);

        // Both rollups in a single atomic transaction so a crash between
        // them can't skip the 1h→1d rollup.
        conn.execute_batch(&format!(
            "BEGIN;
             INSERT OR REPLACE INTO metrics_1h (ts, metric, value)
                SELECT (ts - (ts % 3600)) as hour_ts, metric, SUM(value)
                FROM metrics_1m WHERE ts < {cutoff_1h}
                GROUP BY hour_ts, metric;
             DELETE FROM metrics_1m WHERE ts < {cutoff_1h};
             UPDATE meta SET value = {now} WHERE key = 'last_rollup_1h';
             INSERT OR REPLACE INTO metrics_1d (ts, metric, value)
                SELECT (ts - (ts % 86400)) as day_ts, metric, SUM(value)
                FROM metrics_1h WHERE ts < {cutoff_1d}
                GROUP BY day_ts, metric;
             DELETE FROM metrics_1h WHERE ts < {cutoff_1d};
             UPDATE meta SET value = {now} WHERE key = 'last_rollup_1d';
             COMMIT;"
        ))
        .await?;

        Ok(())
    }

    /// Query metrics for a time range. Automatically selects the best resolution tier.
    pub async fn query_metrics(
        &self,
        metric: &str,
        from: i64,
        to: i64,
    ) -> Result<Vec<MetricPoint>, StoreError> {
        let range = to - from;
        let table = if range <= 86400 {
            "metrics_1m"
        } else if range <= 30 * 86400 {
            "metrics_1h"
        } else {
            "metrics_1d"
        };

        let conn = self.conn()?;
        let mut rows = conn
            .query(
                &format!(
                    "SELECT ts, metric, value FROM {table} WHERE metric = ?1 AND ts >= ?2 AND ts <= ?3 ORDER BY ts"
                ),
                libsql::params![metric, from, to],
            )
            .await?;

        let mut points = Vec::new();
        while let Some(row) = rows.next().await? {
            points.push(MetricPoint {
                ts: row.get(0)?,
                metric: row.get(1)?,
                value: row.get(2)?,
            });
        }
        Ok(points)
    }
}
