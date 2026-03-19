use std::sync::Arc;

use solana_client::nonblocking::rpc_client::RpcClient;
use solana_commitment_config::CommitmentConfig;
use solana_instruction::AccountMeta;
use solana_keypair::Keypair;
use solana_message::{VersionedMessage, v0};
use solana_pubkey::Pubkey;
use solana_signature::Signature;
use solana_signer::Signer;
use solana_transaction::versioned::VersionedTransaction;
use spl_associated_token_account::get_associated_token_address;
use spl_tollbooth_core::error::PaymentError;

use crate::rate_limit::RateLimiter;
use crate::validation::TransactionValidator;
use crate::{Relayer, TokenInfo};

pub struct BuiltinRelayerConfig {
    pub keypair: Arc<Keypair>,
    pub rpc_url: String,
    pub allowed_mints: Vec<Pubkey>,
    pub allowed_recipients: Vec<Pubkey>,
    pub max_transfer_amount: u64,
    pub requests_per_minute: u32,
    pub recipient: Pubkey,
    pub decimals: u8,
}

pub struct BuiltinRelayer {
    keypair: Arc<Keypair>,
    rpc_client: Arc<RpcClient>,
    validator: TransactionValidator,
    rate_limiter: RateLimiter,
    allowed_mints: Vec<Pubkey>,
    recipient: Pubkey,
    decimals: u8,
}

impl BuiltinRelayer {
    pub fn new(config: BuiltinRelayerConfig) -> Result<Self, PaymentError> {
        if config.allowed_mints.is_empty() {
            return Err(PaymentError::RelayError(
                "allowed_mints must not be empty".into(),
            ));
        }
        if config.allowed_recipients.is_empty() {
            return Err(PaymentError::RelayError(
                "allowed_recipients must not be empty".into(),
            ));
        }
        let rpc_client = Arc::new(RpcClient::new_with_commitment(
            config.rpc_url,
            CommitmentConfig::confirmed(),
        ));
        let validator = TransactionValidator::new(
            config.allowed_recipients,
            config.allowed_mints.clone(),
            config.max_transfer_amount,
        );
        let rate_limiter = RateLimiter::new(config.requests_per_minute);

        Ok(Self {
            keypair: config.keypair,
            rpc_client,
            validator,
            rate_limiter,
            allowed_mints: config.allowed_mints,
            recipient: config.recipient,
            decimals: config.decimals,
        })
    }

    pub fn rpc_client(&self) -> &Arc<RpcClient> {
        &self.rpc_client
    }

    pub fn rate_limiter(&self) -> &RateLimiter {
        &self.rate_limiter
    }

    /// Build and fee-payer-sign a transfer transaction for the client to counter-sign.
    /// Returns (serialized tx bytes, reference pubkey).
    pub async fn prepare_transaction(
        &self,
        payer: &Pubkey,
        amount_raw: u64,
    ) -> Result<(Vec<u8>, Pubkey), PaymentError> {
        // 1. Validate amount
        if self.validator.max_transfer_amount() > 0
            && amount_raw > self.validator.max_transfer_amount()
        {
            return Err(PaymentError::RelayError(format!(
                "amount {amount_raw} exceeds max {}",
                self.validator.max_transfer_amount()
            )));
        }

        let mint = self.allowed_mints[0];

        // 2. Derive ATAs
        let sender_ata = get_associated_token_address(payer, &mint);
        let recipient_ata = get_associated_token_address(&self.recipient, &mint);

        // 3. Ensure recipient ATA exists (idempotent — no-op if it already exists)
        let create_recipient_ata_ix =
            spl_associated_token_account::instruction::create_associated_token_account_idempotent(
                &self.keypair.pubkey(), // payer for ATA rent
                &self.recipient,
                &mint,
                &spl_token::id(),
            );

        // 4. Build TransferChecked instruction
        let mut ix = spl_token::instruction::transfer_checked(
            &spl_token::id(),
            &sender_ata,
            &mint,
            &recipient_ata,
            payer,
            &[],
            amount_raw,
            self.decimals,
        )
        .map_err(|e| PaymentError::RelayError(format!("build instruction: {e}")))?;

        // 4. Append reference key
        let reference = Keypair::new();
        ix.accounts.push(AccountMeta {
            pubkey: reference.pubkey(),
            is_signer: false,
            is_writable: false,
        });

        // 5. Fetch blockhash
        let blockhash = self
            .rpc_client
            .get_latest_blockhash()
            .await
            .map_err(|e| PaymentError::RpcError(format!("blockhash: {e}")))?;

        // 6. Compile v0 message
        let message = v0::Message::try_compile(
            &self.keypair.pubkey(),
            &[create_recipient_ata_ix, ix],
            &[],
            blockhash,
        )
        .map_err(|e| PaymentError::RelayError(format!("compile message: {e}")))?;

        // 7. Create transaction with placeholder signatures, then sign fee payer slot only.
        // try_new() requires all signers, but we only have the fee payer at this point.
        // The client will fill in their signature after counter-signing.
        let vm = VersionedMessage::V0(message);
        let num_signers = vm.header().num_required_signatures as usize;
        let message_bytes = vm.serialize();
        let fee_payer_sig = self.keypair.sign_message(&message_bytes);
        let mut signatures = vec![solana_signature::Signature::default(); num_signers];
        signatures[0] = fee_payer_sig;
        let tx = VersionedTransaction {
            signatures,
            message: vm,
        };

        // 8. Serialize
        let bytes = bincode::serialize(&tx)
            .map_err(|e| PaymentError::RelayError(format!("serialize: {e}")))?;

        Ok((bytes, reference.pubkey()))
    }
}

impl Relayer for BuiltinRelayer {
    async fn sign_and_send(&self, tx: &VersionedTransaction) -> Result<Signature, PaymentError> {
        // 1. Validate transaction (recipient allowlist, amount limits)
        let _validation = self
            .validator
            .validate(tx, &self.keypair.pubkey())
            .map_err(|e| PaymentError::RelayError(format!("validation failed: {e}")))?;

        // 2. Verify blockhash is recent
        let blockhash = tx.message.recent_blockhash();
        let is_valid = self
            .rpc_client
            .is_blockhash_valid(blockhash, CommitmentConfig::confirmed())
            .await
            .map_err(|e| PaymentError::RelayError(format!("blockhash check failed: {e}")))?;
        if !is_valid {
            return Err(PaymentError::RelayError(
                "transaction blockhash is expired".into(),
            ));
        }

        // 3. Clone and sign as fee payer
        let mut tx = tx.clone();
        let message_bytes = tx.message.serialize();
        let fee_payer_sig = self.keypair.sign_message(&message_bytes);
        tx.signatures[0] = fee_payer_sig;

        // 4. Submit with retry (up to 3 attempts with exponential backoff)
        let sig = spl_tollbooth_core::retry::retry_transient(3, 100, || {
            self.rpc_client.send_and_confirm_transaction(&tx)
        })
        .await
        .map_err(|e| PaymentError::RelayError(format!("submit failed: {e}")))?;

        Ok(sig)
    }

    fn fee_payer(&self) -> Pubkey {
        self.keypair.pubkey()
    }

    async fn supported_tokens(&self) -> Result<Vec<TokenInfo>, PaymentError> {
        Ok(self
            .allowed_mints
            .iter()
            .map(|mint| TokenInfo {
                mint: *mint,
                symbol: String::new(),
                decimals: self.decimals,
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_keypair::Keypair;
    use solana_signer::Signer;

    fn test_relayer(recipient: Pubkey) -> BuiltinRelayer {
        let keypair = Arc::new(Keypair::new());
        let mint = Pubkey::new_unique();
        BuiltinRelayer {
            keypair,
            rpc_client: Arc::new(
                solana_client::nonblocking::rpc_client::RpcClient::new_with_commitment(
                    "http://localhost:8899".to_string(),
                    solana_commitment_config::CommitmentConfig::confirmed(),
                ),
            ),
            validator: crate::validation::TransactionValidator::new(
                vec![recipient],
                vec![mint],
                1_000_000,
            ),
            rate_limiter: crate::rate_limit::RateLimiter::new(60),
            allowed_mints: vec![mint],
            recipient,
            decimals: 6,
        }
    }

    #[test]
    fn prepare_rejects_excessive_amount() {
        let recipient = Pubkey::new_unique();
        let relayer = test_relayer(recipient);
        let payer = Keypair::new();

        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(relayer.prepare_transaction(&payer.pubkey(), 2_000_000));
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("exceeds"),
            "expected amount limit error, got: {err}"
        );
    }
}
