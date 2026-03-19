pub mod builtin;
pub mod disabled;
pub mod external;
pub mod rate_limit;
pub mod validation;

use solana_pubkey::Pubkey;
use solana_signature::Signature;
use solana_transaction::versioned::VersionedTransaction;
use spl_tollbooth_core::error::PaymentError;

use crate::builtin::BuiltinRelayer;
use crate::disabled::DisabledRelayer;
use crate::external::ExternalRelayer;

/// Fee relayer abstraction. Edition 2024 supports async fn in trait natively.
pub trait Relayer: Send + Sync + 'static {
    /// Atomically sign as fee payer AND submit to network.
    fn sign_and_send(
        &self,
        tx: &VersionedTransaction,
    ) -> impl std::future::Future<Output = Result<Signature, PaymentError>> + Send;

    /// Get the fee payer's public key.
    fn fee_payer(&self) -> Pubkey;

    /// Get supported fee tokens.
    fn supported_tokens(
        &self,
    ) -> impl std::future::Future<Output = Result<Vec<TokenInfo>, PaymentError>> + Send;
}

#[derive(Debug, Clone)]
pub struct TokenInfo {
    pub mint: Pubkey,
    pub symbol: String,
    pub decimals: u8,
}

/// Concrete relayer dispatch enum. Since the `Relayer` trait uses `impl Future`
/// (not object-safe), we use an enum to dispatch to the concrete types.
#[allow(clippy::large_enum_variant)] // Created once at startup, held in Arc
pub enum RelayerKind {
    Builtin(BuiltinRelayer),
    External(ExternalRelayer),
    Disabled(DisabledRelayer),
}

impl RelayerKind {
    pub async fn sign_and_send(
        &self,
        tx: &VersionedTransaction,
    ) -> Result<Signature, PaymentError> {
        match self {
            RelayerKind::Builtin(r) => r.sign_and_send(tx).await,
            RelayerKind::External(r) => r.sign_and_send(tx).await,
            RelayerKind::Disabled(r) => r.sign_and_send(tx).await,
        }
    }

    pub fn fee_payer(&self) -> Pubkey {
        match self {
            RelayerKind::Builtin(r) => r.fee_payer(),
            RelayerKind::External(r) => r.fee_payer(),
            RelayerKind::Disabled(r) => r.fee_payer(),
        }
    }

    pub async fn supported_tokens(&self) -> Result<Vec<TokenInfo>, PaymentError> {
        match self {
            RelayerKind::Builtin(r) => r.supported_tokens().await,
            RelayerKind::External(r) => r.supported_tokens().await,
            RelayerKind::Disabled(r) => r.supported_tokens().await,
        }
    }

    /// Access the rate limiter (only available for BuiltinRelayer).
    pub fn rate_limiter(&self) -> Option<&rate_limit::RateLimiter> {
        match self {
            RelayerKind::Builtin(r) => Some(r.rate_limiter()),
            _ => None,
        }
    }

    pub async fn prepare_transaction(
        &self,
        payer: &Pubkey,
        amount_raw: u64,
    ) -> Result<(Vec<u8>, Pubkey), PaymentError> {
        match self {
            RelayerKind::Builtin(r) => r.prepare_transaction(payer, amount_raw).await,
            _ => Err(PaymentError::RelayError("prepare not supported".into())),
        }
    }

    /// Access the RPC client (only available for BuiltinRelayer).
    pub fn rpc_client(
        &self,
    ) -> Option<&std::sync::Arc<solana_client::nonblocking::rpc_client::RpcClient>> {
        match self {
            RelayerKind::Builtin(r) => Some(r.rpc_client()),
            _ => None,
        }
    }

    pub fn is_disabled(&self) -> bool {
        matches!(self, RelayerKind::Disabled(_))
    }
}
