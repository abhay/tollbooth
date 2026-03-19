use thiserror::Error;

#[derive(Debug, Error)]
pub enum PaymentError {
    #[error("verification failed: {0}")]
    VerificationFailed(String),

    #[error("replay detected: signature {0} already consumed")]
    ReplayDetected(String),

    #[error("insufficient balance: need {need}, have {have}")]
    InsufficientBalance { need: u64, have: u64 },

    #[error("session not found: {0}")]
    SessionNotFound(String),

    #[error("session closed: {0}")]
    SessionClosed(String),

    #[error("invalid bearer token")]
    InvalidBearer,

    #[error("relay error: {0}")]
    RelayError(String),

    #[error("rpc error: {0}")]
    RpcError(String),

    #[error("store error: {0}")]
    StoreError(#[from] StoreError),
}

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Database(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("conflict: {0}")]
    Conflict(String),
}

impl From<libsql::Error> for StoreError {
    fn from(e: libsql::Error) -> Self {
        StoreError::Database(e.to_string())
    }
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("parse error: {0}")]
    Parse(String),

    #[error("validation error: {0}")]
    Validation(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("invalid amount format: {0}")]
    InvalidFormat(String),

    #[error("excess precision: {fractional_digits} fractional digits for {decimals} decimal token")]
    ExcessPrecision {
        fractional_digits: usize,
        decimals: u8,
    },
}
