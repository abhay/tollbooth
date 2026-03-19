use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tokio::sync::mpsc;

use crate::store::LibsqlStore;

/// Spawn a background task that runs metric rollups every 60 seconds.
pub fn spawn_rollup_task(store: Arc<LibsqlStore>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            if let Err(e) = store.rollup_metrics().await {
                tracing::warn!("metrics rollup failed: {e}");
            }
        }
    })
}

/// Response body for GET /metrics.
#[derive(Debug, Serialize)]
pub struct MetricsSummary {
    pub uptime_seconds: u64,
    pub totals: serde_json::Value,
    pub last_24h: Vec<serde_json::Value>,
}

/// A single metric event to be batched.
struct MetricEvent {
    name: String,
    value: f64,
}

/// Batching collector that accumulates metric increments in-memory and flushes
/// them to the store once per second, collapsing duplicate keys into a single
/// DB write. This replaces the previous pattern of spawning a tokio task per
/// metric increment on the hot path.
#[derive(Clone)]
pub struct MetricsCollector {
    tx: mpsc::UnboundedSender<MetricEvent>,
}

impl MetricsCollector {
    /// Create a new collector that flushes to `store` every second.
    /// Spawns a background drain loop on the current tokio runtime.
    pub fn new(store: Arc<LibsqlStore>) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(Self::drain_loop(store, rx));
        Self { tx }
    }

    /// Record a metric increment. Fire-and-forget: silently drops if the
    /// background loop has shut down.
    pub fn increment(&self, name: &str, value: f64) {
        let _ = self.tx.send(MetricEvent {
            name: name.to_string(),
            value,
        });
    }

    /// Background loop: collect events from the channel and flush accumulated
    /// values to the store every second.
    async fn drain_loop(store: Arc<LibsqlStore>, mut rx: mpsc::UnboundedReceiver<MetricEvent>) {
        use std::collections::HashMap;

        let mut interval = tokio::time::interval(Duration::from_secs(1));
        let mut batch: HashMap<String, f64> = HashMap::new();

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if !batch.is_empty() {
                        for (name, value) in batch.drain() {
                            if let Err(e) = store.increment_metric(&name, value).await {
                                tracing::warn!("metric flush failed: {e}");
                            }
                        }
                    }
                }
                event = rx.recv() => {
                    match event {
                        Some(e) => {
                            *batch.entry(e.name).or_default() += e.value;
                        }
                        None => {
                            // Channel closed. Flush remaining and exit.
                            for (name, value) in batch.drain() {
                                if let Err(e) = store.increment_metric(&name, value).await {
                                    tracing::warn!("metric flush failed on shutdown: {e}");
                                }
                            }
                            break;
                        }
                    }
                }
            }
        }
    }
}
