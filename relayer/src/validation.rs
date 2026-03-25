use std::collections::HashSet;

use solana_pubkey::Pubkey;
use solana_transaction::versioned::VersionedTransaction;
use spl_tollbooth_core::error::PaymentError;

/// Configuration for validating relay transactions.
#[derive(Debug, Clone)]
pub struct TransactionValidator {
    /// Raw wallet pubkeys + their derived ATAs for every allowed mint.
    allowed_destinations: HashSet<Pubkey>,
    max_transfer_amount: u64,
}

impl TransactionValidator {
    pub fn new(
        allowed_recipients: Vec<Pubkey>,
        allowed_mints: Vec<Pubkey>,
        max_transfer_amount: u64,
    ) -> Self {
        let mut allowed_destinations: HashSet<Pubkey> =
            allowed_recipients.iter().copied().collect();
        for r in &allowed_recipients {
            for m in &allowed_mints {
                allowed_destinations.insert(
                    spl_associated_token_account::get_associated_token_address(r, m),
                );
            }
        }
        Self {
            allowed_destinations,
            max_transfer_amount,
        }
    }

    pub fn max_transfer_amount(&self) -> u64 {
        self.max_transfer_amount
    }

    /// Validate a partially-signed transaction before the relayer signs as fee payer.
    /// Checks: fee payer slot, recipient in allowlist, amount within limits.
    pub fn validate(
        &self,
        tx: &VersionedTransaction,
        expected_fee_payer: &Pubkey,
    ) -> Result<ValidationResult, PaymentError> {
        let message = &tx.message;

        // Reject transactions with address lookup tables — ALT-resolved accounts
        // bypass our static account validation.
        if let solana_message::VersionedMessage::V0(ref m) = tx.message
            && !m.address_table_lookups.is_empty()
        {
            return Err(PaymentError::RelayError(
                "transactions with address lookup tables are not supported".into(),
            ));
        }

        // Check that the transaction has the expected fee payer
        let account_keys = message.static_account_keys();
        if account_keys.is_empty() {
            return Err(PaymentError::RelayError(
                "transaction has no accounts".into(),
            ));
        }
        if account_keys[0] != *expected_fee_payer {
            return Err(PaymentError::RelayError(format!(
                "fee payer mismatch: expected {}, got {}",
                expected_fee_payer, account_keys[0]
            )));
        }

        // Extract the payer (second signer, the token authority)
        let payer = if account_keys.len() > 1 {
            account_keys[1]
        } else {
            return Err(PaymentError::RelayError(
                "transaction has no token authority".into(),
            ));
        };

        // Parse instructions to find SPL token transfers
        let spl_token_id = spl_token::id();
        let mut transfers = Vec::new();

        for instruction in message.instructions() {
            let program_id = account_keys
                .get(instruction.program_id_index as usize)
                .ok_or_else(|| PaymentError::RelayError("invalid program_id_index".into()))?;

            if *program_id == spl_token_id
                && let Some((amount, dest_index)) =
                    decode_spl_transfer(&instruction.data, &instruction.accounts)
            {
                let dest_account = account_keys.get(dest_index as usize).ok_or_else(|| {
                    PaymentError::RelayError("invalid destination account index".into())
                })?;

                // Enforce recipient allowlist (raw pubkeys + pre-computed ATAs).
                if !self.allowed_destinations.is_empty()
                    && !self.allowed_destinations.contains(dest_account)
                {
                    return Err(PaymentError::RelayError(format!(
                        "recipient {} not in allowlist",
                        dest_account
                    )));
                }

                // Enforce max transfer amount per transfer
                if self.max_transfer_amount > 0 && amount > self.max_transfer_amount {
                    return Err(PaymentError::RelayError(format!(
                        "transfer amount {amount} exceeds max_transfer_amount {}",
                        self.max_transfer_amount
                    )));
                }

                transfers.push(ValidatedTransfer {
                    recipient: *dest_account,
                    amount,
                });
            }
        }

        if transfers.is_empty() {
            return Err(PaymentError::RelayError(
                "no SPL token transfer instruction found".into(),
            ));
        }

        Ok(ValidationResult { payer, transfers })
    }
}

/// Result of validating a relay transaction.
#[derive(Debug)]
pub struct ValidationResult {
    pub payer: Pubkey,
    pub transfers: Vec<ValidatedTransfer>,
}

#[derive(Debug)]
pub struct ValidatedTransfer {
    pub recipient: Pubkey,
    pub amount: u64,
}

/// Try to decode an SPL Token transfer instruction.
/// Returns (amount, destination_account_index) if successful.
fn decode_spl_transfer(data: &[u8], accounts: &[u8]) -> Option<(u64, u8)> {
    if data.is_empty() {
        return None;
    }

    match data[0] {
        // Transfer (instruction type 3)
        3 if data.len() >= 9 && accounts.len() >= 3 => {
            let amount = u64::from_le_bytes(data[1..9].try_into().ok()?);
            let dest_index = accounts[1]; // source=0, dest=1, authority=2
            Some((amount, dest_index))
        }
        // TransferChecked (instruction type 12)
        12 if data.len() >= 9 && accounts.len() >= 4 => {
            let amount = u64::from_le_bytes(data[1..9].try_into().ok()?);
            let dest_index = accounts[2]; // source=0, mint=1, dest=2, authority=3
            Some((amount, dest_index))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_hash::Hash;
    use solana_message::{MessageHeader, VersionedMessage, v0};
    use solana_transaction::CompiledInstruction;

    fn make_transfer_tx(
        fee_payer: Pubkey,
        authority: Pubkey,
        source_ata: Pubkey,
        dest_ata: Pubkey,
        amount: u64,
    ) -> VersionedTransaction {
        let mut data = vec![3u8]; // Transfer instruction
        data.extend_from_slice(&amount.to_le_bytes());

        let accounts = vec![
            fee_payer,       // 0: fee payer
            authority,       // 1: token authority
            source_ata,      // 2: source token account
            dest_ata,        // 3: destination token account
            spl_token::id(), // 4: token program
        ];

        let instruction = CompiledInstruction {
            program_id_index: 4,
            accounts: vec![2, 3, 1], // source, dest, authority
            data,
        };

        let message = v0::Message {
            header: MessageHeader {
                num_required_signatures: 2,
                num_readonly_signed_accounts: 0,
                num_readonly_unsigned_accounts: 1,
            },
            recent_blockhash: Hash::new_unique(),
            account_keys: accounts,
            instructions: vec![instruction],
            address_table_lookups: vec![],
        };

        VersionedTransaction {
            signatures: vec![Default::default(); 2],
            message: VersionedMessage::V0(message),
        }
    }

    #[test]
    fn validate_correct_transfer() {
        let fee_payer = Pubkey::new_unique();
        let authority = Pubkey::new_unique();
        let source_ata = Pubkey::new_unique();
        let dest_ata = Pubkey::new_unique();

        let validator = TransactionValidator::new(vec![dest_ata], vec![], 10_000);
        let tx = make_transfer_tx(fee_payer, authority, source_ata, dest_ata, 1000);
        let result = validator.validate(&tx, &fee_payer).unwrap();
        assert_eq!(result.transfers.len(), 1);
        assert_eq!(result.transfers[0].amount, 1000);
        assert_eq!(result.transfers[0].recipient, dest_ata);
        assert_eq!(result.payer, authority);
    }

    #[test]
    fn reject_wrong_fee_payer() {
        let fee_payer = Pubkey::new_unique();
        let wrong_payer = Pubkey::new_unique();
        let authority = Pubkey::new_unique();
        let source_ata = Pubkey::new_unique();
        let dest_ata = Pubkey::new_unique();

        let validator = TransactionValidator::new(vec![dest_ata], vec![], 10_000);
        let tx = make_transfer_tx(fee_payer, authority, source_ata, dest_ata, 1000);
        let result = validator.validate(&tx, &wrong_payer);
        assert!(result.is_err());
    }

    #[test]
    fn reject_unlisted_recipient() {
        let fee_payer = Pubkey::new_unique();
        let authority = Pubkey::new_unique();
        let source_ata = Pubkey::new_unique();
        let dest_ata = Pubkey::new_unique();
        let allowed_ata = Pubkey::new_unique(); // different from dest

        let validator = TransactionValidator::new(vec![allowed_ata], vec![], 10_000);
        let tx = make_transfer_tx(fee_payer, authority, source_ata, dest_ata, 1000);
        let result = validator.validate(&tx, &fee_payer);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not in allowlist"));
    }

    #[test]
    fn reject_excessive_amount() {
        let fee_payer = Pubkey::new_unique();
        let authority = Pubkey::new_unique();
        let source_ata = Pubkey::new_unique();
        let dest_ata = Pubkey::new_unique();

        let validator = TransactionValidator::new(vec![dest_ata], vec![], 500);
        let tx = make_transfer_tx(fee_payer, authority, source_ata, dest_ata, 1000);
        let result = validator.validate(&tx, &fee_payer);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("exceeds max_transfer_amount")
        );
    }

    #[test]
    fn reject_unlisted_recipient_in_multi_transfer() {
        let fee_payer = Pubkey::new_unique();
        let authority = Pubkey::new_unique();
        let source_ata = Pubkey::new_unique();
        let allowed_ata = Pubkey::new_unique();
        let attacker_ata = Pubkey::new_unique();

        // Build a tx with two SPL transfer instructions:
        // 1st: large transfer to attacker_ata (not in allowlist — should be rejected)
        // 2nd: tiny transfer to allowed_ata
        let mut data_big = vec![3u8]; // Transfer instruction
        data_big.extend_from_slice(&500_000u64.to_le_bytes());

        let mut data_small = vec![3u8]; // Transfer instruction
        data_small.extend_from_slice(&1u64.to_le_bytes());

        let accounts = vec![
            fee_payer,       // 0: fee payer
            authority,       // 1: token authority
            source_ata,      // 2: source token account
            attacker_ata,    // 3: attacker destination
            allowed_ata,     // 4: allowed destination
            spl_token::id(), // 5: token program
        ];

        let ix_big = CompiledInstruction {
            program_id_index: 5,
            accounts: vec![2, 3, 1], // source, attacker_dest, authority
            data: data_big,
        };

        let ix_small = CompiledInstruction {
            program_id_index: 5,
            accounts: vec![2, 4, 1], // source, allowed_dest, authority
            data: data_small,
        };

        let message = v0::Message {
            header: MessageHeader {
                num_required_signatures: 2,
                num_readonly_signed_accounts: 0,
                num_readonly_unsigned_accounts: 1,
            },
            recent_blockhash: Hash::new_unique(),
            account_keys: accounts,
            instructions: vec![ix_big, ix_small],
            address_table_lookups: vec![],
        };

        let tx = VersionedTransaction {
            signatures: vec![Default::default(); 2],
            message: VersionedMessage::V0(message),
        };

        let validator = TransactionValidator::new(vec![allowed_ata], vec![], 1_000_000);
        let result = validator.validate(&tx, &fee_payer);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not in allowlist"));
    }

    #[test]
    fn accept_multiple_transfers_all_allowed() {
        let fee_payer = Pubkey::new_unique();
        let authority = Pubkey::new_unique();
        let source_ata = Pubkey::new_unique();
        let dest_ata_1 = Pubkey::new_unique();
        let dest_ata_2 = Pubkey::new_unique();

        let mut data1 = vec![3u8];
        data1.extend_from_slice(&1000u64.to_le_bytes());

        let mut data2 = vec![3u8];
        data2.extend_from_slice(&2000u64.to_le_bytes());

        let accounts = vec![
            fee_payer,       // 0: fee payer
            authority,       // 1: token authority
            source_ata,      // 2: source token account
            dest_ata_1,      // 3: destination 1
            dest_ata_2,      // 4: destination 2
            spl_token::id(), // 5: token program
        ];

        let ix1 = CompiledInstruction {
            program_id_index: 5,
            accounts: vec![2, 3, 1],
            data: data1,
        };

        let ix2 = CompiledInstruction {
            program_id_index: 5,
            accounts: vec![2, 4, 1],
            data: data2,
        };

        let message = v0::Message {
            header: MessageHeader {
                num_required_signatures: 2,
                num_readonly_signed_accounts: 0,
                num_readonly_unsigned_accounts: 1,
            },
            recent_blockhash: Hash::new_unique(),
            account_keys: accounts,
            instructions: vec![ix1, ix2],
            address_table_lookups: vec![],
        };

        let tx = VersionedTransaction {
            signatures: vec![Default::default(); 2],
            message: VersionedMessage::V0(message),
        };

        let validator = TransactionValidator::new(vec![dest_ata_1, dest_ata_2], vec![], 1_000_000);
        let result = validator.validate(&tx, &fee_payer).unwrap();
        assert_eq!(result.transfers.len(), 2);
        assert_eq!(result.transfers[0].recipient, dest_ata_1);
        assert_eq!(result.transfers[0].amount, 1000);
        assert_eq!(result.transfers[1].recipient, dest_ata_2);
        assert_eq!(result.transfers[1].amount, 2000);
    }

    #[test]
    fn empty_allowlist_permits_any_recipient() {
        let fee_payer = Pubkey::new_unique();
        let authority = Pubkey::new_unique();
        let source_ata = Pubkey::new_unique();
        let dest_ata = Pubkey::new_unique();

        let validator = TransactionValidator::new(vec![], vec![], 10_000);
        let tx = make_transfer_tx(fee_payer, authority, source_ata, dest_ata, 1000);
        assert!(validator.validate(&tx, &fee_payer).is_ok());
    }
}
