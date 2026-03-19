use std::sync::Arc;

use hmac::{Hmac, Mac};
use sha2::Sha256;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_keypair::Keypair;
use solana_message::v0;
use solana_pubkey::Pubkey;
use solana_signature::Signature;
use solana_signer::Signer;
use solana_transaction::versioned::VersionedTransaction;
use spl_tollbooth_core::error::PaymentError;
use spl_tollbooth_core::store::LibsqlStore;
use spl_tollbooth_core::store::signatures::ConsumeResult;
use spl_tollbooth_core::types::{
    PaymentReceipt, ProtocolKind, SessionState, SessionStatus, TokenAmount,
};

use crate::context::MppContext;
use crate::types::{MppSessionChallenge, MppSessionCredential};
use crate::verify;

pub struct MppSession {
    pub ctx: MppContext,
    pub server_keypair: Arc<Keypair>,
    /// Cached keypair bytes for HMAC bearer derivation (computed once in `new()`).
    bearer_key: [u8; 64],
}

impl MppSession {
    pub fn new(ctx: MppContext, server_keypair: Arc<Keypair>) -> Self {
        let bearer_key = server_keypair.to_bytes();
        Self {
            ctx,
            server_keypair,
            bearer_key,
        }
    }

    /// Generate a session challenge.
    pub fn challenge(&self, deposit: &TokenAmount) -> MppSessionChallenge {
        MppSessionChallenge {
            deposit: deposit.display(),
            recipient: self.ctx.recipient.to_string(),
            mint: self.ctx.mint.to_string(),
            decimals: self.ctx.decimals,
            relay_url: self.ctx.relay_url.clone(),
            fee_payer: self.ctx.relayer_pubkey.map(|p| p.to_string()),
        }
    }

    /// Process a session credential.
    pub async fn process(
        &self,
        credential: &MppSessionCredential,
        cost: u64,
    ) -> Result<PaymentReceipt, PaymentError> {
        match credential {
            MppSessionCredential::Open {
                signature,
                refund_address,
                bearer,
            } => self.open(signature, refund_address, bearer, cost).await,
            MppSessionCredential::Bearer { session_id, bearer } => {
                self.bearer(session_id, bearer, cost).await
            }
            MppSessionCredential::TopUp {
                session_id,
                signature,
            } => self.top_up(session_id, signature).await,
            MppSessionCredential::Close { session_id, bearer } => {
                self.close(session_id, bearer).await
            }
        }
    }

    /// Open a new session by verifying the deposit transaction.
    async fn open(
        &self,
        deposit_signature: &str,
        refund_address: &str,
        client_bearer: &str,
        min_deposit: u64,
    ) -> Result<PaymentReceipt, PaymentError> {
        // Fast-path: already consumed → return cached receipt
        if let Some(cached) = self
            .ctx
            .store
            .check_consumed_receipt(deposit_signature)
            .await?
        {
            return Ok(cached);
        }

        // Validate refund_address is a valid Solana pubkey
        let _refund_pubkey: solana_pubkey::Pubkey = refund_address.parse().map_err(|e| {
            PaymentError::VerificationFailed(format!("invalid refund address: {e}"))
        })?;

        let result = verify::find_and_verify_transfer(
            &self.ctx.rpc_client,
            deposit_signature,
            &self.ctx.recipient,
            &self.ctx.mint,
            self.ctx.decimals,
        )
        .await?;

        // Enforce minimum deposit
        if min_deposit > 0 && result.amount < min_deposit {
            return Err(PaymentError::InsufficientBalance {
                need: min_deposit,
                have: result.amount,
            });
        }

        // The client generates a random bearer secret and includes it in the Open credential.
        // We store HMAC(key, bearer) so we can verify later without storing the raw bearer.
        let bearer_hash = derive_bearer_hash(&self.bearer_key, client_bearer);
        let session_id = uuid::Uuid::new_v4().to_string();
        let now = now_secs();

        // Build receipt (needed for consume_idempotent)
        let receipt = PaymentReceipt {
            protocol: ProtocolKind::Mpp,
            signature: deposit_signature.to_string(),
            amount: spl_tollbooth_core::types::display_amount(result.amount, self.ctx.decimals),
            mint: self.ctx.mint.to_string(),
            payer: result.payer.to_string(),
            recipient: self.ctx.recipient.to_string(),
            timestamp: now,
            session_id: Some(session_id.clone()),
        };

        // Consume FIRST: prevents double-open if two requests race with the same deposit sig
        match self
            .ctx
            .store
            .consume_idempotent(
                deposit_signature,
                "mpp-session-deposit",
                Some(&result.amount.to_string()),
                Some(&result.payer.to_string()),
                &receipt,
            )
            .await?
        {
            ConsumeResult::Consumed(r) => {
                // We won the consume race — create the session
                let session = SessionState {
                    session_id: session_id.clone(),
                    bearer_hash,
                    deposit_amount: result.amount,
                    spent: 0,
                    refund_address: refund_address.to_string(),
                    mint: self.ctx.mint.to_string(),
                    decimals: self.ctx.decimals,
                    status: SessionStatus::Active,
                    refund_signature: None,
                    created_at: now,
                    updated_at: now,
                };
                self.ctx.store.create_session(&session).await?;
                Ok(r)
            }
            ConsumeResult::AlreadyCached(r) => {
                // Another request already created the session, return cached receipt
                Ok(r)
            }
        }
    }

    /// Verify a bearer token against the stored session hash.
    async fn verify_bearer(
        &self,
        session_id: &str,
        bearer: &str,
    ) -> Result<SessionState, PaymentError> {
        let session = self
            .ctx
            .store
            .get_session(session_id)
            .await?
            .ok_or_else(|| PaymentError::SessionNotFound(session_id.to_string()))?;

        let provided_hash = derive_bearer_hash(&self.bearer_key, bearer);
        if provided_hash != session.bearer_hash {
            return Err(PaymentError::InvalidBearer);
        }
        Ok(session)
    }

    /// Validate bearer token and deduct cost from session balance.
    async fn bearer(
        &self,
        session_id: &str,
        bearer: &str,
        cost: u64,
    ) -> Result<PaymentReceipt, PaymentError> {
        // Compute bearer hash for verification
        let bearer_hash = derive_bearer_hash(&self.bearer_key, bearer);

        // Single-query: debit with bearer hash verification
        let debit_result = self
            .ctx
            .store
            .debit_session(session_id, cost, now_secs(), &bearer_hash)
            .await?;

        let session = match debit_result {
            Some(s) => s,
            None => {
                // Debit failed: disambiguation on the error path (not hot path).
                // Re-fetch to distinguish bearer mismatch, not-active, or insufficient balance.
                let current = self.ctx.store.get_session(session_id).await?;
                return Err(match current {
                    Some(s) => {
                        let provided_hash = derive_bearer_hash(&self.bearer_key, bearer);
                        if provided_hash != s.bearer_hash {
                            PaymentError::InvalidBearer
                        } else if s.status != SessionStatus::Active {
                            PaymentError::SessionClosed(session_id.to_string())
                        } else {
                            PaymentError::InsufficientBalance {
                                need: cost,
                                have: s.deposit_amount.saturating_sub(s.spent),
                            }
                        }
                    }
                    None => PaymentError::SessionNotFound(session_id.to_string()),
                });
            }
        };

        Ok(PaymentReceipt {
            protocol: ProtocolKind::Mpp,
            signature: format!("session:{session_id}"),
            amount: spl_tollbooth_core::types::display_amount(cost, self.ctx.decimals),
            mint: session.mint,
            payer: session.refund_address.clone(),
            recipient: self.ctx.recipient.to_string(),
            timestamp: session.updated_at,
            session_id: Some(session_id.to_string()),
        })
    }

    /// Top up an existing session with additional funds.
    async fn top_up(
        &self,
        session_id: &str,
        topup_signature: &str,
    ) -> Result<PaymentReceipt, PaymentError> {
        // Fast-path: already consumed → return cached receipt without updating session
        if let Some(cached) = self
            .ctx
            .store
            .check_consumed_receipt(topup_signature)
            .await?
        {
            // Verify the cached receipt belongs to THIS session
            if cached.session_id.as_deref() != Some(session_id) {
                return Err(PaymentError::ReplayDetected(
                    "signature already used for a different session".into(),
                ));
            }
            return Ok(cached);
        }

        let result = verify::find_and_verify_transfer(
            &self.ctx.rpc_client,
            topup_signature,
            &self.ctx.recipient,
            &self.ctx.mint,
            self.ctx.decimals,
        )
        .await?;

        // Build receipt
        let receipt = PaymentReceipt {
            protocol: ProtocolKind::Mpp,
            signature: topup_signature.to_string(),
            amount: spl_tollbooth_core::types::display_amount(result.amount, self.ctx.decimals),
            mint: self.ctx.mint.to_string(),
            payer: result.payer.to_string(),
            recipient: self.ctx.recipient.to_string(),
            timestamp: now_secs(),
            session_id: Some(session_id.to_string()),
        };

        // Consume FIRST: prevents double-credit if two requests race
        match self
            .ctx
            .store
            .consume_idempotent(
                topup_signature,
                "mpp-session-topup",
                Some(&result.amount.to_string()),
                Some(&result.payer.to_string()),
                &receipt,
            )
            .await?
        {
            ConsumeResult::AlreadyCached(r) => {
                // Another request already credited the session, return cached receipt
                Ok(r)
            }
            ConsumeResult::Consumed(r) => {
                // We won the consume race — atomically credit the session
                self.ctx
                    .store
                    .credit_session(session_id, result.amount, now_secs())
                    .await?
                    .ok_or_else(|| PaymentError::SessionClosed(session_id.to_string()))?;
                Ok(r)
            }
        }
    }

    /// Close a session and refund remaining balance.
    async fn close(&self, session_id: &str, bearer: &str) -> Result<PaymentReceipt, PaymentError> {
        self.verify_bearer(session_id, bearer).await?;

        // Atomically transition active → closing (prevents double-close)
        let session = self
            .ctx
            .store
            .begin_close_session(session_id, now_secs())
            .await?
            .ok_or_else(|| PaymentError::SessionClosed(session_id.to_string()))?;

        let refund_amount = session.deposit_amount.saturating_sub(session.spent);
        let mut refund_sig_str = None;

        if refund_amount > 0 {
            let refund_to: Pubkey = session
                .refund_address
                .parse()
                .map_err(|e| PaymentError::RelayError(format!("invalid refund address: {e}")))?;

            let sig = build_and_send_refund(
                &self.ctx.rpc_client,
                &self.server_keypair,
                &self.ctx.mint,
                self.ctx.decimals,
                &self.ctx.recipient,
                &refund_to,
                refund_amount,
            )
            .await?;

            refund_sig_str = Some(sig.to_string());

            // Atomically transition closing → closed with refund signature
            self.ctx
                .store
                .finalize_close_session(session_id, refund_sig_str.as_deref(), now_secs())
                .await?;

            tracing::info!(
                session_id,
                refund_amount,
                refund_signature = %sig,
                "session closed with refund"
            );
        } else {
            // Atomically transition closing → closed (no refund needed)
            self.ctx
                .store
                .finalize_close_session(session_id, None, now_secs())
                .await?;
            tracing::info!(session_id, "session closed (no refund needed)");
        }

        Ok(PaymentReceipt {
            protocol: ProtocolKind::Mpp,
            signature: refund_sig_str.unwrap_or_else(|| format!("session-close:{session_id}")),
            amount: spl_tollbooth_core::types::display_amount(refund_amount, self.ctx.decimals),
            mint: session.mint,
            payer: session.refund_address,
            recipient: self.ctx.recipient.to_string(),
            timestamp: session.updated_at,
            session_id: Some(session_id.to_string()),
        })
    }
}

/// Build and send an SPL token refund transaction.
/// Includes idempotent ATA creation for the recipient in case their account was closed.
pub async fn build_and_send_refund(
    rpc_client: &RpcClient,
    server_keypair: &Keypair,
    mint: &Pubkey,
    decimals: u8,
    from_wallet: &Pubkey,
    to_wallet: &Pubkey,
    amount: u64,
) -> Result<Signature, PaymentError> {
    let from_ata = spl_associated_token_account::get_associated_token_address(from_wallet, mint);
    let to_ata = spl_associated_token_account::get_associated_token_address(to_wallet, mint);

    // Idempotent ATA creation; handles the case where recipient's ATA was closed
    let create_ata_ix =
        spl_associated_token_account::instruction::create_associated_token_account_idempotent(
            &server_keypair.pubkey(), // payer for ATA rent
            to_wallet,
            mint,
            &spl_token::id(),
        );

    let transfer_ix = spl_token::instruction::transfer_checked(
        &spl_token::id(),
        &from_ata,
        mint,
        &to_ata,
        &server_keypair.pubkey(), // authority (server holds custody)
        &[],
        amount,
        decimals,
    )
    .map_err(|e| PaymentError::RelayError(format!("build transfer ix: {e}")))?;

    let blockhash = rpc_client
        .get_latest_blockhash()
        .await
        .map_err(|e| PaymentError::RpcError(format!("get blockhash: {e}")))?;

    // Build v0 message (no address lookup tables needed for simple transfers)
    let v0_msg = v0::Message::try_compile(
        &server_keypair.pubkey(),
        &[create_ata_ix, transfer_ix],
        &[], // no address lookup tables
        blockhash,
    )
    .map_err(|e| PaymentError::RelayError(format!("compile v0 message: {e}")))?;

    let tx = VersionedTransaction::try_new(
        solana_message::VersionedMessage::V0(v0_msg),
        &[server_keypair],
    )
    .map_err(|e| PaymentError::RelayError(format!("sign versioned tx: {e}")))?;

    // Submit with retry
    let sig = spl_tollbooth_core::retry::retry_transient(3, 200, || {
        rpc_client.send_and_confirm_transaction(&tx)
    })
    .await
    .map_err(|e| PaymentError::RelayError(format!("refund submit failed: {e}")))?;

    Ok(sig)
}

/// Derive a bearer hash using HMAC-SHA256 with the server secret.
/// This prevents attackers from deriving bearer hashes from public on-chain signatures,
/// since the server secret is required.
fn derive_bearer_hash(secret: &[u8], bearer: &str) -> String {
    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(bearer.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

use spl_tollbooth_core::types::now_secs;

/// Recover stuck sessions on startup (crash recovery).
/// For sessions stuck in `Closing`:
/// - If `refund_signature` exists: check if confirmed, mark closed
/// - If no `refund_signature`: re-attempt the refund
pub async fn recover_stuck_sessions(
    store: &LibsqlStore,
    rpc_client: &RpcClient,
    keypair: &Keypair,
) -> Result<(), PaymentError> {
    let stuck = store.find_closing_sessions().await?;
    for session in stuck {
        let mint: Pubkey = session
            .mint
            .parse()
            .map_err(|e| PaymentError::RpcError(format!("invalid session mint: {e}")))?;
        let refund_to: Pubkey = session
            .refund_address
            .parse()
            .map_err(|e| PaymentError::RpcError(format!("invalid refund address: {e}")))?;
        // The recipient wallet is the server's custody address (derived from keypair)
        let server_wallet = keypair.pubkey();

        let refund_amount = session.deposit_amount.saturating_sub(session.spent);

        if let Some(ref sig_str) = session.refund_signature {
            // Refund was attempted, check if it landed
            let signature: solana_signature::Signature = sig_str
                .parse()
                .map_err(|e| PaymentError::RpcError(format!("invalid refund sig: {e}")))?;
            match rpc_client.confirm_transaction(&signature).await {
                Ok(true) => {
                    // Already confirmed on-chain — just finalize in DB
                    store
                        .finalize_close_session(
                            &session.session_id,
                            session.refund_signature.as_deref(),
                            now_secs(),
                        )
                        .await?;
                    tracing::info!(
                        session_id = session.session_id,
                        "recovered stuck session (refund confirmed)"
                    );
                }
                Ok(false) => {
                    // Definitively not confirmed — safe to re-attempt refund
                    tracing::warn!(
                        session_id = session.session_id,
                        "refund {sig_str} not confirmed, re-attempting"
                    );
                    match build_and_send_refund(
                        rpc_client,
                        keypair,
                        &mint,
                        session.decimals,
                        &server_wallet,
                        &refund_to,
                        refund_amount,
                    )
                    .await
                    {
                        Ok(new_sig) => {
                            store
                                .finalize_close_session(
                                    &session.session_id,
                                    Some(&new_sig.to_string()),
                                    now_secs(),
                                )
                                .await?;
                            tracing::info!(
                                session_id = session.session_id,
                                refund_signature = %new_sig,
                                "recovered stuck session (re-attempted refund)"
                            );
                        }
                        Err(e) => {
                            tracing::error!(
                                session_id = session.session_id,
                                "crash recovery refund failed: {e}"
                            );
                        }
                    }
                }
                Err(e) => {
                    // RPC error — skip this session to avoid double-refund risk.
                    // Will retry on the next recovery cycle.
                    tracing::warn!(
                        session_id = session.session_id,
                        "RPC error checking refund status, skipping: {e}"
                    );
                    continue;
                }
            }
        } else {
            // No refund signature at all, first attempt
            tracing::warn!(
                session_id = session.session_id,
                "stuck session with no refund signature, attempting refund"
            );
            match build_and_send_refund(
                rpc_client,
                keypair,
                &mint,
                session.decimals,
                &server_wallet,
                &refund_to,
                refund_amount,
            )
            .await
            {
                Ok(sig) => {
                    store
                        .finalize_close_session(
                            &session.session_id,
                            Some(&sig.to_string()),
                            now_secs(),
                        )
                        .await?;
                    tracing::info!(
                        session_id = session.session_id,
                        refund_signature = %sig,
                        "recovered stuck session (new refund)"
                    );
                }
                Err(e) => {
                    tracing::error!(
                        session_id = session.session_id,
                        "crash recovery refund failed: {e}"
                    );
                }
            }
        }
    }
    Ok(())
}
