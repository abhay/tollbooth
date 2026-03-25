mod common;

use spl_tollbooth_core::store::signatures::ConsumeResult;
use spl_tollbooth_core::types::{PaymentReceipt, ProtocolKind, SessionState, SessionStatus};

fn dummy_receipt(sig: &str) -> PaymentReceipt {
    PaymentReceipt {
        protocol: ProtocolKind::Mpp,
        signature: sig.to_string(),
        amount: "1000".to_string(),
        ui_amount: "0.001".to_string(),
        mint: "mint1".to_string(),
        payer: "payer1".to_string(),
        recipient: "recipient1".to_string(),
        timestamp: 1000,
        session_id: None,
    }
}

#[tokio::test]
async fn mark_and_check_consumed() {
    let (store, _dir) = common::test_store().await;

    // Not consumed yet
    assert!(
        store
            .check_consumed_receipt("sig123")
            .await
            .unwrap()
            .is_none()
    );

    let receipt = dummy_receipt("sig123");
    let result = store
        .consume_idempotent(
            "sig123",
            "mpp-charge",
            Some("1000"),
            Some("payer1"),
            &receipt,
        )
        .await
        .unwrap();
    assert!(matches!(result, ConsumeResult::Consumed(_)));

    // Now consumed
    assert!(
        store
            .check_consumed_receipt("sig123")
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn double_mark_is_already_cached() {
    let (store, _dir) = common::test_store().await;

    let receipt = dummy_receipt("sig123");
    let result = store
        .consume_idempotent("sig123", "mpp-charge", None, None, &receipt)
        .await
        .unwrap();
    assert!(matches!(result, ConsumeResult::Consumed(_)));

    // Second consume returns AlreadyCached, not an error
    let result = store
        .consume_idempotent("sig123", "mpp-charge", None, None, &receipt)
        .await
        .unwrap();
    assert!(matches!(result, ConsumeResult::AlreadyCached(_)));
}

#[tokio::test]
async fn create_and_get_session() {
    let (store, _dir) = common::test_store().await;

    let session = SessionState {
        session_id: "sess-1".into(),
        bearer_hash: "abc123".into(),
        deposit_amount: 100_000,
        spent: 0,
        refund_address: "refund-addr".into(),
        mint: "mint-addr".into(),
        decimals: 6,
        status: SessionStatus::Active,
        refund_signature: None,
        created_at: 1000,
        updated_at: 1000,
    };

    store.create_session(&session).await.unwrap();
    let loaded = store.get_session("sess-1").await.unwrap().unwrap();
    assert_eq!(loaded.deposit_amount, 100_000);
    assert_eq!(loaded.status, SessionStatus::Active);
}

#[tokio::test]
async fn update_session_spent() {
    let (store, _dir) = common::test_store().await;

    let mut session = SessionState {
        session_id: "sess-2".into(),
        bearer_hash: "abc".into(),
        deposit_amount: 100_000,
        spent: 0,
        refund_address: "addr".into(),
        mint: "mint".into(),
        decimals: 6,
        status: SessionStatus::Active,
        refund_signature: None,
        created_at: 1000,
        updated_at: 1000,
    };

    store.create_session(&session).await.unwrap();
    session.spent = 10_000;
    session.updated_at = 2000;
    store.update_session(&session).await.unwrap();

    let loaded = store.get_session("sess-2").await.unwrap().unwrap();
    assert_eq!(loaded.spent, 10_000);
}

#[tokio::test]
async fn find_stuck_closing_sessions() {
    let (store, _dir) = common::test_store().await;

    let session = SessionState {
        session_id: "stuck".into(),
        bearer_hash: "abc".into(),
        deposit_amount: 100_000,
        spent: 50_000,
        refund_address: "addr".into(),
        mint: "mint".into(),
        decimals: 6,
        status: SessionStatus::Closing,
        refund_signature: None,
        created_at: 1000,
        updated_at: 1000,
    };
    store.create_session(&session).await.unwrap();

    let stuck = store.find_closing_sessions().await.unwrap();
    assert_eq!(stuck.len(), 1);
    assert_eq!(stuck[0].session_id, "stuck");
}

// ---------------------------------------------------------------------------
// Atomic session fund management
// ---------------------------------------------------------------------------

fn active_session(id: &str, deposit: u64) -> SessionState {
    SessionState {
        session_id: id.into(),
        bearer_hash: "hash".into(),
        deposit_amount: deposit,
        spent: 0,
        refund_address: "addr".into(),
        mint: "mint".into(),
        decimals: 6,
        status: SessionStatus::Active,
        refund_signature: None,
        created_at: 1000,
        updated_at: 1000,
    }
}

#[tokio::test]
async fn debit_session_succeeds_with_sufficient_balance() {
    let (store, _dir) = common::test_store().await;
    store
        .create_session(&active_session("s1", 100_000))
        .await
        .unwrap();

    let result = store
        .debit_session("s1", 30_000, 2000, "hash")
        .await
        .unwrap();
    assert!(result.is_some());
    let s = result.unwrap();
    assert_eq!(s.spent, 30_000);
    assert_eq!(s.deposit_amount, 100_000);

    // Debit again — balance is now 70k remaining
    let result = store
        .debit_session("s1", 70_000, 3000, "hash")
        .await
        .unwrap();
    assert!(result.is_some());
    assert_eq!(result.unwrap().spent, 100_000);
}

#[tokio::test]
async fn debit_session_fails_with_insufficient_balance() {
    let (store, _dir) = common::test_store().await;
    store
        .create_session(&active_session("s1", 100_000))
        .await
        .unwrap();

    // Try to debit more than deposit
    let result = store
        .debit_session("s1", 100_001, 2000, "hash")
        .await
        .unwrap();
    assert!(result.is_none());

    // Balance unchanged
    let s = store.get_session("s1").await.unwrap().unwrap();
    assert_eq!(s.spent, 0);
}

#[tokio::test]
async fn debit_session_fails_for_non_active_session() {
    let (store, _dir) = common::test_store().await;

    let mut session = active_session("s1", 100_000);
    session.status = SessionStatus::Closed;
    store.create_session(&session).await.unwrap();

    let result = store
        .debit_session("s1", 1_000, 2000, "hash")
        .await
        .unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn credit_session_adds_to_deposit() {
    let (store, _dir) = common::test_store().await;
    store
        .create_session(&active_session("s1", 100_000))
        .await
        .unwrap();

    let result = store.credit_session("s1", 50_000, 2000).await.unwrap();
    assert!(result.is_some());
    assert_eq!(result.unwrap().deposit_amount, 150_000);
}

#[tokio::test]
async fn credit_session_fails_for_closed_session() {
    let (store, _dir) = common::test_store().await;

    let mut session = active_session("s1", 100_000);
    session.status = SessionStatus::Closing;
    store.create_session(&session).await.unwrap();

    let result = store.credit_session("s1", 50_000, 2000).await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn begin_close_transitions_active_to_closing() {
    let (store, _dir) = common::test_store().await;
    store
        .create_session(&active_session("s1", 100_000))
        .await
        .unwrap();

    let result = store.begin_close_session("s1", 2000).await.unwrap();
    assert!(result.is_some());
    assert_eq!(result.unwrap().status, SessionStatus::Closing);

    // Verify in DB
    let s = store.get_session("s1").await.unwrap().unwrap();
    assert_eq!(s.status, SessionStatus::Closing);
}

#[tokio::test]
async fn begin_close_rejects_double_close() {
    let (store, _dir) = common::test_store().await;
    store
        .create_session(&active_session("s1", 100_000))
        .await
        .unwrap();

    // First close succeeds
    let result = store.begin_close_session("s1", 2000).await.unwrap();
    assert!(result.is_some());

    // Second close returns None (already closing)
    let result = store.begin_close_session("s1", 3000).await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn debit_after_partial_spend_respects_remaining_balance() {
    let (store, _dir) = common::test_store().await;
    store
        .create_session(&active_session("s1", 100_000))
        .await
        .unwrap();

    // Spend 60k
    store
        .debit_session("s1", 60_000, 2000, "hash")
        .await
        .unwrap()
        .unwrap();

    // Try to spend 50k more — only 40k remaining, should fail
    let result = store
        .debit_session("s1", 50_000, 3000, "hash")
        .await
        .unwrap();
    assert!(result.is_none());

    // Spend exactly 40k — should succeed
    let result = store
        .debit_session("s1", 40_000, 4000, "hash")
        .await
        .unwrap();
    assert!(result.is_some());
    assert_eq!(result.unwrap().spent, 100_000);
}
