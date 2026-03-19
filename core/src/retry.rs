use std::future::Future;
use std::time::Duration;

/// Retry an async operation up to `max_attempts` times with exponential backoff.
/// Only retries on transient errors (503, 429, timeout in the error string).
pub async fn retry_transient<F, Fut, T, E>(
    max_attempts: u32,
    base_delay_ms: u64,
    mut op: F,
) -> Result<T, String>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    E: std::fmt::Display,
{
    let mut last_err = String::new();
    for attempt in 0..max_attempts {
        match op().await {
            Ok(val) => return Ok(val),
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("503") || err_str.contains("429") || err_str.contains("timeout")
                {
                    tracing::warn!(attempt, "transient error, retrying: {err_str}");
                    last_err = err_str;
                    tokio::time::sleep(Duration::from_millis(base_delay_ms * 2u64.pow(attempt)))
                        .await;
                    continue;
                }
                return Err(err_str);
            }
        }
    }
    Err(format!("failed after {max_attempts} attempts: {last_err}"))
}
