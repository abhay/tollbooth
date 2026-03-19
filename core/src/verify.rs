use solana_client::nonblocking::rpc_client::RpcClient;
use solana_client::rpc_config::RpcTransactionConfig;
use solana_commitment_config::CommitmentConfig;
use solana_pubkey::Pubkey;
use solana_signature::Signature;
use solana_transaction_status::option_serializer::OptionSerializer;

use crate::error::PaymentError;

/// Result of verifying an on-chain transfer.
#[derive(Debug)]
pub struct VerifiedTransfer {
    pub amount: u64,
    pub payer: Pubkey,
    pub recipient: Pubkey,
    pub mint: Pubkey,
}

/// Find and verify an SPL token transfer on-chain.
pub async fn find_and_verify_transfer(
    rpc_client: &RpcClient,
    signature_str: &str,
    expected_recipient: &Pubkey,
    expected_mint: &Pubkey,
    _decimals: u8,
) -> Result<VerifiedTransfer, PaymentError> {
    let signature: Signature = signature_str
        .parse()
        .map_err(|e| PaymentError::VerificationFailed(format!("invalid signature: {e}")))?;

    let config = RpcTransactionConfig {
        // Using "confirmed" commitment (~80% validator vote, ~0.4s) rather than "finalized"
        // (~13s). This is a latency/safety trade-off: confirmed transactions are very rarely
        // reverted on mainnet, but a sophisticated attacker could exploit a short fork to
        // deposit, open a session, and have the deposit rolled back. For high-value deployments,
        // change this to CommitmentConfig::finalized(). The max_transfer_amount cap limits
        // exposure under the current setting.
        commitment: Some(CommitmentConfig::confirmed()),
        max_supported_transaction_version: Some(0),
        ..Default::default()
    };

    let tx = fetch_transaction_with_retry(rpc_client, &signature, config).await?;

    let meta =
        tx.transaction.meta.as_ref().ok_or_else(|| {
            PaymentError::VerificationFailed("transaction has no metadata".into())
        })?;

    if meta.err.is_some() {
        return Err(PaymentError::VerificationFailed(
            "transaction failed on-chain".into(),
        ));
    }

    // Extract pre/post token balances using OptionSerializer
    let pre_balances = match &meta.pre_token_balances {
        OptionSerializer::Some(v) => v,
        _ => {
            return Err(PaymentError::VerificationFailed(
                "no pre_token_balances".into(),
            ));
        }
    };
    let post_balances = match &meta.post_token_balances {
        OptionSerializer::Some(v) => v,
        _ => {
            return Err(PaymentError::VerificationFailed(
                "no post_token_balances".into(),
            ));
        }
    };

    // Find recipient's token account balance change
    for post_balance in post_balances.iter() {
        let owner_str = match &post_balance.owner {
            OptionSerializer::Some(s) => s.clone(),
            _ => continue,
        };
        let owner_pubkey: Pubkey = match owner_str.parse() {
            Ok(p) => p,
            Err(_) => continue,
        };
        if owner_pubkey != *expected_recipient {
            continue;
        }

        // Check mint
        let mint_pubkey: Pubkey = match post_balance.mint.parse() {
            Ok(p) => p,
            Err(_) => continue,
        };
        if mint_pubkey != *expected_mint {
            continue;
        }

        let post_amount: u64 = post_balance.ui_token_amount.amount.parse().unwrap_or(0);

        // Find matching pre-balance
        let pre_amount: u64 = pre_balances
            .iter()
            .find(|b| b.account_index == post_balance.account_index)
            .map(|b| b.ui_token_amount.amount.parse().unwrap_or(0))
            .unwrap_or(0);

        if post_amount > pre_amount {
            let amount = post_amount - pre_amount;

            // Find the payer by looking at who lost tokens
            let payer = find_sender(pre_balances, post_balances, expected_mint).unwrap_or_default();

            return Ok(VerifiedTransfer {
                amount,
                payer,
                recipient: *expected_recipient,
                mint: *expected_mint,
            });
        }
    }

    Err(PaymentError::VerificationFailed(
        "no matching transfer found in transaction".into(),
    ))
}

pub fn find_sender(
    pre: &[solana_transaction_status::UiTransactionTokenBalance],
    post: &[solana_transaction_status::UiTransactionTokenBalance],
    expected_mint: &Pubkey,
) -> Option<Pubkey> {
    for pre_balance in pre.iter() {
        let mint_pubkey: Pubkey = pre_balance.mint.parse().ok()?;
        if mint_pubkey != *expected_mint {
            continue;
        }

        let pre_amount: u64 = pre_balance.ui_token_amount.amount.parse().ok()?;
        let post_amount: u64 = post
            .iter()
            .find(|b| b.account_index == pre_balance.account_index)
            .and_then(|b| b.ui_token_amount.amount.parse().ok())
            .unwrap_or(0);

        if pre_amount > post_amount
            && let OptionSerializer::Some(ref owner) = pre_balance.owner
        {
            return owner.parse().ok();
        }
    }
    None
}

async fn fetch_transaction_with_retry(
    rpc_client: &RpcClient,
    signature: &Signature,
    config: RpcTransactionConfig,
) -> Result<solana_transaction_status::EncodedConfirmedTransactionWithStatusMeta, PaymentError> {
    crate::retry::retry_transient(3, 200, || {
        rpc_client.get_transaction_with_config(signature, config)
    })
    .await
    .map_err(PaymentError::RpcError)
}
