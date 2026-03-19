use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Json;
use serde::{Deserialize, Serialize};

use crate::state::AppState;

#[derive(Serialize)]
pub struct RelayResponse {
    pub signature: String,
}

#[derive(Serialize)]
pub struct RelayError {
    pub error: String,
}

#[derive(Deserialize)]
pub struct PrepareRequest {
    pub payer: String,
    pub amount: String,
}

/// POST /relay: accept a partially-signed transaction, validate, sign as fee payer, submit.
pub async fn relay_handler(
    State(state): State<AppState>,
    body: Bytes,
) -> Result<Json<RelayResponse>, (StatusCode, Json<RelayError>)> {
    // 1. Get the relayer and check if disabled
    let relayer = state.relayer.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(RelayError {
                error: "relay not configured".into(),
            }),
        )
    })?;
    if relayer.is_disabled() {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(RelayError {
                error: "relay is disabled".into(),
            }),
        ));
    }

    // 2. Increment relay.requests metric
    state.metrics.increment("relay.requests", 1.0);

    // 3. Deserialize the transaction
    let tx: solana_transaction::versioned::VersionedTransaction = bincode::deserialize(&body)
        .map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(RelayError {
                    error: format!("invalid transaction: {e}"),
                }),
            )
        })?;

    // Rate limit by client pubkey. For prepared transactions, account[1] is always
    // the token authority (payer). For externally-built transactions, this is the
    // second signer which is typically the token authority by convention.
    let account_keys = tx.message.static_account_keys();
    let client_key = account_keys
        .get(1)
        .map(|k| k.to_string())
        .unwrap_or_default();
    if let Some(limiter) = relayer.rate_limiter()
        && !limiter.check(&client_key)
    {
        state.metrics.increment("relay.rejected", 1.0);
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            Json(RelayError {
                error: "rate limit exceeded".into(),
            }),
        ));
    }

    // 5. Sign and submit
    let signature = relayer.sign_and_send(&tx).await.map_err(|e| {
        let err_msg = e.to_string();
        tracing::warn!("relay error: {err_msg}");
        state.metrics.increment("relay.rejected", 1.0);
        state.metrics.increment("errors.relay", 1.0);
        let (status, client_msg) =
            if err_msg.contains("validation failed") || err_msg.contains("expired") {
                (StatusCode::BAD_REQUEST, "transaction rejected".to_string())
            } else if err_msg.contains("already been processed") {
                // Solana TransactionError::AlreadyProcessed — transaction landed in a
                // prior attempt but the client didn't get the response.
                (
                    StatusCode::CONFLICT,
                    "transaction already processed".to_string(),
                )
            } else {
                (
                    StatusCode::BAD_GATEWAY,
                    format!("relay submission failed: {err_msg}"),
                )
            };
        (status, Json(RelayError { error: client_msg }))
    })?;

    // 6. Increment success metrics
    state.metrics.increment("relay.signed", 1.0);

    Ok(Json(RelayResponse {
        signature: signature.to_string(),
    }))
}

/// POST /relay/prepare: build and fee-payer-sign a transaction for the client to counter-sign.
pub async fn prepare_handler(
    State(state): State<AppState>,
    axum::Json(req): axum::Json<PrepareRequest>,
) -> Result<
    (
        StatusCode,
        [(axum::http::header::HeaderName, String); 1],
        Vec<u8>,
    ),
    (StatusCode, Json<RelayError>),
> {
    // 1. Get the relayer
    let relayer = state.relayer.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(RelayError {
                error: "relay not configured".into(),
            }),
        )
    })?;

    // 2. Check relayer supports prepare
    if relayer.is_disabled() {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(RelayError {
                error: "relay is disabled".into(),
            }),
        ));
    }

    // 3. Parse payer pubkey
    let payer: solana_pubkey::Pubkey = req.payer.parse().map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(RelayError {
                error: format!("invalid payer pubkey: {}", req.payer),
            }),
        )
    })?;

    // 4. Parse amount to raw u64
    let amount = spl_tollbooth_core::types::TokenAmount::from_display(
        &req.amount,
        state.mint,
        state.decimals,
    )
    .map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(RelayError {
                error: format!("invalid amount: {e}"),
            }),
        )
    })?;

    // 5. Rate limit by payer pubkey
    let client_key = payer.to_string();
    if let Some(limiter) = relayer.rate_limiter()
        && !limiter.check(&client_key)
    {
        state.metrics.increment("relay.rejected", 1.0);
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            Json(RelayError {
                error: "rate limit exceeded".into(),
            }),
        ));
    }

    // 6. Build and sign the transaction
    let (tx_bytes, reference) = relayer
        .prepare_transaction(&payer, amount.raw)
        .await
        .map_err(|e| {
            tracing::warn!("prepare error: {e}");
            state.metrics.increment("errors.relay", 1.0);
            let err_msg = e.to_string();
            let status = if err_msg.contains("exceeds") {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::BAD_GATEWAY
            };
            (
                status,
                Json(RelayError {
                    error: format!("prepare failed: {err_msg}"),
                }),
            )
        })?;

    // 7. Metrics
    state.metrics.increment("relay.prepare", 1.0);

    // 8. Return binary response with reference header
    Ok((
        StatusCode::OK,
        [(
            axum::http::header::HeaderName::from_static("x-reference"),
            reference.to_string(),
        )],
        tx_bytes,
    ))
}
