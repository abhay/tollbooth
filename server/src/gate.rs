use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use spl_tollbooth_core::config::{RouteEntry, RouteMode};
use spl_tollbooth_core::types::{PaymentReceipt, ProtocolKind, TokenAmount};
use spl_tollbooth_mpp::types::MppSessionCredential;

use crate::headers;
use crate::state::AppState;
use crate::util::enqueue_webhook;

/// Outcome of the payment gate check.
pub enum GateResult {
    /// Payment verified: receipt and event type to attach to response.
    Verified(PaymentReceipt, String),
    /// No valid credential present; return 402 challenge.
    Challenge(Response),
    /// Verification failed. Return error response (metrics already incremented).
    #[allow(dead_code)]
    Failed(Response),
}

/// Check payment credentials against the configured protocol and route.
///
/// `price_raw` is the authoritative price in raw token units (already parsed by the caller).
/// On success returns `GateResult::Verified` with the receipt and event type.
/// On missing/invalid credentials returns `GateResult::Challenge` or `GateResult::Failed`.
pub async fn check_payment(
    state: &AppState,
    headers: &HeaderMap,
    route: &RouteEntry,
    _path: &str,
    protocol: ProtocolKind,
    price_raw: u64,
) -> GateResult {
    match protocol {
        ProtocolKind::Mpp => check_mpp(state, headers, route, price_raw).await,
    }
}

/// MPP protocol: try session credential, then charge proof, then return 402 challenge.
async fn check_mpp(
    state: &AppState,
    headers: &HeaderMap,
    route: &RouteEntry,
    price_raw: u64,
) -> GateResult {
    // Try session credential first (for session routes)
    if route.mode == RouteMode::Session
        && let Some(cred_header) = headers.get(headers::X_PAYMENT_CREDENTIAL)
        && let Ok(cred_str) = cred_header.to_str()
        && let Some(ref session) = state.mpp_session
    {
        match serde_json::from_str::<MppSessionCredential>(cred_str) {
            Ok(credential) => {
                // For Open credentials, enforce the deposit amount (not per-request price).
                let cost = if matches!(credential, MppSessionCredential::Open { .. }) {
                    if let Some(ref deposit_str) = route.deposit {
                        TokenAmount::from_display(deposit_str, state.mint, state.decimals)
                            .map(|t| t.raw)
                            .unwrap_or(price_raw)
                    } else {
                        price_raw
                    }
                } else {
                    price_raw
                };
                match session.process(&credential, cost).await {
                    Ok(receipt) => {
                        let event_type = match &credential {
                            MppSessionCredential::Open { .. } => "session.opened",
                            MppSessionCredential::Close { .. } => "session.closed",
                            _ => "payment.completed",
                        };
                        enqueue_webhook(state, event_type, &receipt);
                        let metric = match &credential {
                            MppSessionCredential::Open { .. } => "payments.mpp.session.open",
                            MppSessionCredential::Close { .. } => "payments.mpp.session.close",
                            _ => "payments.mpp.session.bearer",
                        };
                        increment_metrics(state, metric, cost);
                        return GateResult::Verified(receipt, event_type.to_string());
                    }
                    Err(e) => {
                        tracing::warn!("MPP session credential failed: {e}");
                        increment_error(state);
                    }
                }
            }
            Err(e) => {
                tracing::warn!("failed to deserialize session credential: {e}");
                increment_error(state);
            }
        }
    }

    // Try X-Payment-Signature header (charge flow)
    if let Some(sig) = headers.get(headers::X_PAYMENT_SIGNATURE)
        && let Ok(sig_str) = sig.to_str()
        && let Some(ref charge) = state.mpp_charge
    {
        let proof = spl_tollbooth_mpp::types::MppChargeProof {
            signature: sig_str.to_string(),
        };
        match charge.verify(&proof, price_raw).await {
            Ok(receipt) => {
                enqueue_webhook(state, "payment.completed", &receipt);
                increment_metrics(state, "payments.mpp.charge", price_raw);
                return GateResult::Verified(receipt, "payment.completed".to_string());
            }
            Err(e) => {
                tracing::warn!("MPP verification failed: {e}");
                increment_error(state);
            }
        }
    }

    // Return 402 challenge
    GateResult::Challenge(mpp_challenge(state, route, price_raw))
}

/// Build the MPP 402 challenge response.
fn mpp_challenge(state: &AppState, route: &RouteEntry, price_raw: u64) -> Response {
    if route.mode == RouteMode::Session {
        if let Some(ref session) = state.mpp_session {
            // Use explicit deposit (display string) if configured, otherwise fall back to price_raw.
            let deposit = if let Some(ref deposit_str) = route.deposit {
                TokenAmount::from_display(deposit_str, state.mint, state.decimals)
                    .unwrap_or_else(|_| TokenAmount::new(0, state.mint, state.decimals))
            } else {
                TokenAmount::new(price_raw, state.mint, state.decimals)
            };
            let challenge = session.challenge(&deposit);
            return (
                StatusCode::PAYMENT_REQUIRED,
                [(headers::X_PAYMENT_PROTOCOL, HeaderValue::from_static("mpp"))],
                axum::Json(challenge),
            )
                .into_response();
        }
    } else if let Some(ref charge) = state.mpp_charge {
        let price = TokenAmount::new(price_raw, state.mint, state.decimals);
        let challenge = charge.challenge(&price);
        return (
            StatusCode::PAYMENT_REQUIRED,
            [(headers::X_PAYMENT_PROTOCOL, HeaderValue::from_static("mpp"))],
            axum::Json(challenge),
        )
            .into_response();
    }

    StatusCode::PAYMENT_REQUIRED.into_response()
}

/// Increment a payment metric and revenue total via the batching collector.
fn increment_metrics(state: &AppState, metric: &str, amount_raw: u64) {
    state.metrics.increment(metric, 1.0);
    state.metrics.increment("revenue.total", amount_raw as f64);
}

/// Increment the verify error metric via the batching collector.
fn increment_error(state: &AppState) {
    state.metrics.increment("errors.verify", 1.0);
}
