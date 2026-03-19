use crate::state::AppState;

/// Enqueue a webhook event (non-blocking, best-effort).
pub(crate) fn enqueue_webhook(
    state: &AppState,
    event_type: &str,
    receipt: &spl_tollbooth_core::types::PaymentReceipt,
) {
    if let Some(ref wh) = state.webhooks {
        if !wh.enabled {
            return;
        }
    } else {
        return;
    }

    let store = state.store.clone();
    let event_type = event_type.to_string();
    let payload = serde_json::to_value(receipt).unwrap_or_default();
    tokio::spawn(async move {
        if let Err(e) = store.enqueue_webhook(&event_type, &payload).await {
            tracing::warn!("failed to enqueue webhook: {e}");
        }
    });
}
