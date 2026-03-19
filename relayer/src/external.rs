use solana_pubkey::Pubkey;
use solana_signature::Signature;
use solana_transaction::versioned::VersionedTransaction;
use spl_tollbooth_core::error::PaymentError;

use crate::{Relayer, TokenInfo};

/// Configuration for the external (Kora) relayer.
pub struct ExternalRelayerConfig {
    pub endpoint: String,
    pub api_key: Option<String>,
    pub fee_payer_pubkey: Pubkey,
}

/// Relayer that talks to an external Kora-compatible JSON-RPC endpoint.
pub struct ExternalRelayer {
    endpoint: String,
    api_key: Option<String>,
    fee_payer_pubkey: Pubkey,
    client: reqwest::Client,
}

impl ExternalRelayer {
    pub fn new(config: ExternalRelayerConfig) -> Result<Self, PaymentError> {
        Ok(Self {
            endpoint: config.endpoint,
            api_key: config.api_key,
            fee_payer_pubkey: config.fee_payer_pubkey,
            client: reqwest::Client::new(),
        })
    }
}

impl Relayer for ExternalRelayer {
    async fn sign_and_send(&self, tx: &VersionedTransaction) -> Result<Signature, PaymentError> {
        let tx_bytes = bincode::serialize(tx)
            .map_err(|e| PaymentError::RelayError(format!("serialize tx: {e}")))?;
        let tx_base64 = base64_encode(&tx_bytes);

        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "signAndSendTransaction",
            "params": [tx_base64]
        });

        let mut request = self.client.post(&self.endpoint).json(&payload);
        if let Some(ref key) = self.api_key {
            request = request.header("Authorization", format!("Bearer {key}"));
        }

        let response = request
            .send()
            .await
            .map_err(|e| PaymentError::RelayError(format!("kora request failed: {e}")))?;

        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| PaymentError::RelayError(format!("kora response parse: {e}")))?;

        if let Some(error) = body.get("error") {
            return Err(PaymentError::RelayError(format!("kora error: {}", error)));
        }

        let sig_str = body["result"]
            .as_str()
            .ok_or_else(|| PaymentError::RelayError("missing result in kora response".into()))?;

        sig_str
            .parse()
            .map_err(|e| PaymentError::RelayError(format!("invalid signature: {e}")))
    }

    fn fee_payer(&self) -> Pubkey {
        self.fee_payer_pubkey
    }

    async fn supported_tokens(&self) -> Result<Vec<TokenInfo>, PaymentError> {
        // External relayer: query the endpoint for supported tokens
        // For now, return empty; operators configure this externally
        Ok(vec![])
    }
}

fn base64_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}
