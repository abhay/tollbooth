use solana_pubkey::Pubkey;
use solana_signature::Signature;
use solana_transaction::versioned::VersionedTransaction;
use spl_tollbooth_core::error::PaymentError;

use crate::{Relayer, TokenInfo};

/// A relayer that does nothing. Clients must pay their own fees.
pub struct DisabledRelayer;

impl Relayer for DisabledRelayer {
    async fn sign_and_send(&self, _tx: &VersionedTransaction) -> Result<Signature, PaymentError> {
        Err(PaymentError::RelayError(
            "relay is disabled, clients must pay their own fees".into(),
        ))
    }

    fn fee_payer(&self) -> Pubkey {
        Pubkey::default()
    }

    async fn supported_tokens(&self) -> Result<Vec<TokenInfo>, PaymentError> {
        Ok(vec![])
    }
}
