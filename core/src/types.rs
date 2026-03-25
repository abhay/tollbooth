use serde::{Deserialize, Serialize};
use solana_pubkey::Pubkey;

use crate::error::ParseError;

/// Token amount. Raw u64 is authoritative for all arithmetic.
/// Display strings are only used at config parse and API serialization boundaries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenAmount {
    pub raw: u64,
    pub mint: Pubkey,
    pub decimals: u8,
}

impl TokenAmount {
    pub fn new(raw: u64, mint: Pubkey, decimals: u8) -> Self {
        Self {
            raw,
            mint,
            decimals,
        }
    }

    /// Parse a human-readable amount (e.g. "0.001") into raw units.
    /// Fails fast on excess precision (e.g. "0.0000001" with 6 decimals).
    pub fn from_display(s: &str, mint: Pubkey, decimals: u8) -> Result<Self, ParseError> {
        if s.is_empty() {
            return Err(ParseError::InvalidFormat("empty string".into()));
        }

        let parts: Vec<&str> = s.split('.').collect();
        match parts.len() {
            1 => {
                let whole: u64 = parts[0]
                    .parse()
                    .map_err(|_| ParseError::InvalidFormat(s.into()))?;
                let raw = whole
                    .checked_mul(10u64.pow(decimals as u32))
                    .ok_or_else(|| ParseError::InvalidFormat("overflow".into()))?;
                Ok(Self {
                    raw,
                    mint,
                    decimals,
                })
            }
            2 => {
                if decimals == 0 {
                    return Err(ParseError::ExcessPrecision {
                        fractional_digits: parts[1].len(),
                        decimals,
                    });
                }
                let fractional_digits = parts[1].len();
                if fractional_digits > decimals as usize {
                    return Err(ParseError::ExcessPrecision {
                        fractional_digits,
                        decimals,
                    });
                }
                let whole: u64 = parts[0]
                    .parse()
                    .map_err(|_| ParseError::InvalidFormat(s.into()))?;
                let frac_str = format!("{:0<width$}", parts[1], width = decimals as usize);
                let frac: u64 = frac_str
                    .parse()
                    .map_err(|_| ParseError::InvalidFormat(s.into()))?;
                let raw = whole
                    .checked_mul(10u64.pow(decimals as u32))
                    .and_then(|w| w.checked_add(frac))
                    .ok_or_else(|| ParseError::InvalidFormat("overflow".into()))?;
                Ok(Self {
                    raw,
                    mint,
                    decimals,
                })
            }
            _ => Err(ParseError::InvalidFormat(s.into())),
        }
    }

    /// Format for human display only. Don't use for arithmetic.
    pub fn display(&self) -> String {
        display_amount(self.raw, self.decimals)
    }
}

/// Format a raw token amount as a human-readable display string.
pub fn display_amount(raw: u64, decimals: u8) -> String {
    if decimals == 0 {
        return raw.to_string();
    }
    let divisor = 10u64.pow(decimals as u32);
    let whole = raw / divisor;
    let frac = raw % divisor;
    if frac == 0 {
        return whole.to_string();
    }
    let frac_str = format!("{:0>width$}", frac, width = decimals as usize);
    let trimmed = frac_str.trim_end_matches('0');
    format!("{whole}.{trimmed}")
}

/// Payment mode for a route.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum PaymentMode {
    Charge,
    Session { deposit: String },
}

/// Route configuration from TOML.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteConfig {
    pub path: String,
    pub method: Option<String>,
    pub price: String,
    #[serde(flatten)]
    pub mode: PaymentMode,
}

/// Protocol kind identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProtocolKind {
    Mpp,
}

impl std::fmt::Display for ProtocolKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProtocolKind::Mpp => write!(f, "mpp"),
        }
    }
}

/// Payment receipt returned after successful verification.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaymentReceipt {
    pub protocol: ProtocolKind,
    pub signature: String,
    /// Amount in raw token units (e.g. "1000000").
    pub amount: String,
    /// Display amount (e.g. "1.0").
    pub ui_amount: String,
    pub mint: String,
    pub payer: String,
    pub recipient: String,
    pub timestamp: i64,
    /// Present for MPP session open/close/top-up operations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

/// Session state stored in the database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    pub session_id: String,
    pub bearer_hash: String,
    pub deposit_amount: u64,
    pub spent: u64,
    pub refund_address: String,
    pub mint: String,
    pub decimals: u8,
    pub status: SessionStatus,
    pub refund_signature: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    Active,
    Closing,
    Closed,
}

/// Webhook entry from the queue.
#[derive(Debug, Clone)]
pub struct WebhookEntry {
    pub id: i64,
    pub event_type: String,
    pub payload: serde_json::Value,
    pub created_at: i64,
    pub attempts: i32,
}

/// Get the current Unix timestamp in seconds.
pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// A single metric data point.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricPoint {
    pub ts: i64,
    pub metric: String,
    pub value: f64,
}
