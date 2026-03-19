use spl_tollbooth_core::error::PaymentError;
use spl_tollbooth_core::store::signatures::ConsumeResult;
use spl_tollbooth_core::types::{PaymentReceipt, ProtocolKind, TokenAmount};

use crate::context::MppContext;
use crate::types::{MppChallenge, MppChargeProof};
use crate::verify;

pub struct MppCharge {
    pub ctx: MppContext,
}

impl MppCharge {
    /// Generate a challenge for an MPP charge request.
    pub fn challenge(&self, price: &TokenAmount) -> MppChallenge {
        MppChallenge {
            amount: price.display(),
            recipient: self.ctx.recipient.to_string(),
            mint: self.ctx.mint.to_string(),
            decimals: self.ctx.decimals,
            relay_url: self.ctx.relay_url.clone(),
            fee_payer: self.ctx.relayer_pubkey.map(|p| p.to_string()),
        }
    }

    /// Verify an MPP charge proof (transaction signature).
    /// Idempotent: retrying with an already-consumed signature returns the cached receipt.
    pub async fn verify(
        &self,
        proof: &MppChargeProof,
        min_amount: u64,
    ) -> Result<PaymentReceipt, PaymentError> {
        // Fast-path: already consumed → return cached receipt
        if let Some(cached) = self
            .ctx
            .store
            .check_consumed_receipt(&proof.signature)
            .await?
        {
            return Ok(cached);
        }

        // Verify on-chain
        let result = verify::find_and_verify_transfer(
            &self.ctx.rpc_client,
            &proof.signature,
            &self.ctx.recipient,
            &self.ctx.mint,
            self.ctx.decimals,
        )
        .await?;

        // Enforce minimum amount
        if min_amount > 0 && result.amount < min_amount {
            return Err(PaymentError::InsufficientBalance {
                need: min_amount,
                have: result.amount,
            });
        }

        // Build receipt
        let receipt = PaymentReceipt {
            protocol: ProtocolKind::Mpp,
            signature: proof.signature.clone(),
            amount: spl_tollbooth_core::types::display_amount(result.amount, self.ctx.decimals),
            mint: self.ctx.mint.to_string(),
            payer: result.payer.to_string(),
            recipient: self.ctx.recipient.to_string(),
            timestamp: spl_tollbooth_core::types::now_secs(),
            session_id: None,
        };

        // Consume idempotently
        match self
            .ctx
            .store
            .consume_idempotent(
                &proof.signature,
                "mpp-charge",
                Some(&result.amount.to_string()),
                Some(&result.payer.to_string()),
                &receipt,
            )
            .await?
        {
            ConsumeResult::Consumed(r) | ConsumeResult::AlreadyCached(r) => Ok(r),
        }
    }
}
