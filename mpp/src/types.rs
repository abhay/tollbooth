use serde::{Deserialize, Serialize};

/// MPP charge challenge, returned in 402 response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MppChallenge {
    pub amount: String,
    pub ui_amount: String,
    pub recipient: String,
    pub mint: String,
    pub decimals: u8,
    pub relay_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fee_payer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform_fee: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform_fee_ui_amount: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform_fee_recipient: Option<String>,
}

/// MPP session challenge, returned in 402 for session routes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MppSessionChallenge {
    pub deposit: String,
    pub deposit_ui_amount: String,
    pub recipient: String,
    pub mint: String,
    pub decimals: u8,
    pub relay_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fee_payer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform_fee: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform_fee_ui_amount: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform_fee_recipient: Option<String>,
}

/// MPP charge proof. Client presents this after payment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MppChargeProof {
    pub signature: String,
}

/// MPP session credential variants.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum MppSessionCredential {
    #[serde(rename_all = "camelCase")]
    Open {
        signature: String,
        refund_address: String,
        bearer: String,
    },
    #[serde(rename_all = "camelCase")]
    Bearer { session_id: String, bearer: String },
    #[serde(rename_all = "camelCase")]
    TopUp {
        session_id: String,
        signature: String,
    },
    #[serde(rename_all = "camelCase")]
    Close { session_id: String, bearer: String },
}
