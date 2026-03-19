use std::sync::Arc;
use std::time::Duration;

use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::config::WebhooksConfig;
use crate::store::LibsqlStore;

type HmacSha256 = Hmac<Sha256>;

/// Compute HMAC-SHA256 signature for a webhook payload.
pub fn compute_hmac(secret: &str, payload: &str) -> String {
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(payload.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// Backoff durations for webhook retry attempts (1s, 5s per spec).
const RETRY_BACKOFFS: [Duration; 3] = [
    Duration::from_secs(0), // first attempt: immediate
    Duration::from_secs(1), // second attempt: 1s
    Duration::from_secs(5), // third attempt: 5s
];

/// Spawn a background task that drains the webhook queue and delivers via HTTP POST.
pub fn spawn_webhook_task(
    store: Arc<LibsqlStore>,
    config: WebhooksConfig,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let client = reqwest::Client::new();
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        let url = match config.url {
            Some(ref u) => u.clone(),
            None => return,
        };
        let secret = config.secret.clone().unwrap_or_default();
        if secret.is_empty() {
            tracing::error!("webhook secret is empty, webhook signatures will be insecure");
        }

        loop {
            interval.tick().await;
            let webhooks = store.dequeue_webhooks(10).await.unwrap_or_default();
            for webhook in webhooks {
                let payload_str = serde_json::to_string(&webhook.payload).unwrap_or_default();
                let signature = compute_hmac(&secret, &payload_str);

                // Apply exponential backoff based on attempt count
                let backoff_index = (webhook.attempts as usize).min(RETRY_BACKOFFS.len() - 1);
                if webhook.attempts > 0 {
                    tokio::time::sleep(RETRY_BACKOFFS[backoff_index]).await;
                }

                let result = client
                    .post(&url)
                    .header("X-Tollbooth-Signature", &signature)
                    .header("X-Tollbooth-Event", &webhook.event_type)
                    .header("Content-Type", "application/json")
                    .body(payload_str)
                    .timeout(Duration::from_secs(5))
                    .send()
                    .await;
                match result {
                    Ok(resp) if resp.status().is_success() => {
                        store.mark_webhook_delivered(webhook.id).await.ok();
                    }
                    _ => {
                        store.mark_webhook_failed(webhook.id).await.ok();
                    }
                }
            }
        }
    })
}
