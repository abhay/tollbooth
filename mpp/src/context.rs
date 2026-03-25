use std::sync::Arc;

use solana_client::nonblocking::rpc_client::RpcClient;
use solana_pubkey::Pubkey;
use spl_tollbooth_core::store::LibsqlStore;

/// Shared context for MPP protocol handlers (charge and session).
pub struct MppContext {
    pub recipient: Pubkey,
    pub mint: Pubkey,
    pub decimals: u8,
    pub rpc_client: Arc<RpcClient>,
    pub store: Arc<LibsqlStore>,
    pub relayer_pubkey: Option<Pubkey>,
    pub relay_url: String,
    pub platform_fee_recipient: Option<Pubkey>,
    pub platform_fee_flat_raw: u64,
    pub platform_fee_percent: f64,
}

impl MppContext {
    /// Compute the platform fee for a given custody amount (in raw units).
    /// Returns 0 if no platform fee is configured.
    pub fn compute_platform_fee(&self, custody_raw: u64) -> u64 {
        if self.platform_fee_recipient.is_none() {
            return 0;
        }
        let flat = self.platform_fee_flat_raw;
        let pct = (custody_raw as f64 * self.platform_fee_percent / 100.0) as u64;
        flat + pct
    }

    /// Verify the platform fee was paid in the given transaction.
    /// Returns Ok(()) if no fee is configured or if the fee transfer is present and sufficient.
    pub async fn verify_platform_fee(
        &self,
        signature: &str,
        custody_amount_raw: u64,
    ) -> Result<(), spl_tollbooth_core::error::PaymentError> {
        let fee_recipient = match self.platform_fee_recipient {
            Some(r) => r,
            None => return Ok(()),
        };
        let expected_fee = self.compute_platform_fee(custody_amount_raw);
        if expected_fee == 0 {
            return Ok(());
        }

        let result = crate::verify::find_and_verify_transfer(
            &self.rpc_client,
            signature,
            &fee_recipient,
            &self.mint,
            self.decimals,
        )
        .await?;

        if result.amount < expected_fee {
            return Err(spl_tollbooth_core::error::PaymentError::VerificationFailed(
                format!(
                    "platform fee insufficient: expected {expected_fee}, got {}",
                    result.amount
                ),
            ));
        }
        Ok(())
    }
}
