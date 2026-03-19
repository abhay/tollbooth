use serde::{Deserialize, Serialize};

use crate::types::ProtocolKind;

/// Challenge response variants. Each protocol uses its native wire format.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum ChallengeResponse {
    MppCharge(serde_json::Value),
    MppSession(serde_json::Value),
}

/// Payment proof variants, dispatched based on protocol negotiation.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum PaymentProof {
    MppCharge { signature: String },
}

impl PaymentProof {
    pub fn protocol_kind(&self) -> ProtocolKind {
        match self {
            PaymentProof::MppCharge { .. } => ProtocolKind::Mpp,
        }
    }
}
