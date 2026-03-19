//! Integration test for session close with refund.
//! Requires a running solana-test-validator on localhost:8899.
//!
//! Run with: cargo test -p spl-tollbooth-mpp --test session_refund_integration -- --ignored

use solana_client::nonblocking::rpc_client::RpcClient;
use solana_commitment_config::CommitmentConfig;
use solana_keypair::Keypair;
use solana_native_token::LAMPORTS_PER_SOL;
use solana_signature::Signature;
use solana_signer::Signer;
use solana_system_interface::instruction as system_instruction;
use solana_transaction::Transaction;

/// Test session close refund:
/// 1. Server keypair holds tokens in its ATA (simulating custody after deposit)
/// 2. Call build_and_send_refund to transfer back to the client
/// 3. Verify client received the refund
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn session_close_sends_refund() {
    let rpc_url = "http://127.0.0.1:8899";
    let rpc = RpcClient::new_with_commitment(rpc_url.to_string(), CommitmentConfig::confirmed());

    let server_kp = Keypair::new();
    let client_kp = Keypair::new();

    // Airdrop SOL to server (pays for tx fees + ATA rent)
    let sig = rpc
        .request_airdrop(&server_kp.pubkey(), 2 * LAMPORTS_PER_SOL)
        .await
        .expect("airdrop server");
    wait(&rpc, &sig).await;

    let sig = rpc
        .request_airdrop(&client_kp.pubkey(), LAMPORTS_PER_SOL)
        .await
        .expect("airdrop client");
    wait(&rpc, &sig).await;

    // Create mint (server is authority)
    let mint_kp = Keypair::new();
    let mint_rent = rpc
        .get_minimum_balance_for_rent_exemption(82)
        .await
        .unwrap();
    let blockhash = rpc.get_latest_blockhash().await.unwrap();
    let tx = Transaction::new_signed_with_payer(
        &[
            system_instruction::create_account(
                &server_kp.pubkey(),
                &mint_kp.pubkey(),
                mint_rent,
                82,
                &spl_token::id(),
            ),
            spl_token::instruction::initialize_mint2(
                &spl_token::id(),
                &mint_kp.pubkey(),
                &server_kp.pubkey(),
                None,
                6,
            )
            .unwrap(),
        ],
        Some(&server_kp.pubkey()),
        &[&server_kp, &mint_kp],
        blockhash,
    );
    rpc.send_and_confirm_transaction(&tx).await.unwrap();

    let mint = mint_kp.pubkey();
    let server_ata =
        spl_associated_token_account::get_associated_token_address(&server_kp.pubkey(), &mint);
    let client_ata =
        spl_associated_token_account::get_associated_token_address(&client_kp.pubkey(), &mint);

    // Create server ATA and mint tokens to it (simulating deposit custody)
    let blockhash = rpc.get_latest_blockhash().await.unwrap();
    let tx = Transaction::new_signed_with_payer(
        &[
            spl_associated_token_account::instruction::create_associated_token_account(
                &server_kp.pubkey(),
                &server_kp.pubkey(),
                &mint,
                &spl_token::id(),
            ),
            spl_token::instruction::mint_to(
                &spl_token::id(),
                &mint,
                &server_ata,
                &server_kp.pubkey(),
                &[],
                10_000_000, // 10 tokens
            )
            .unwrap(),
        ],
        Some(&server_kp.pubkey()),
        &[&server_kp],
        blockhash,
    );
    rpc.send_and_confirm_transaction(&tx).await.unwrap();

    // Verify server has 10 tokens
    let bal = rpc.get_token_account_balance(&server_ata).await.unwrap();
    assert_eq!(bal.amount, "10000000");

    // --- Test refund: server sends 7 tokens back to client ---
    // Note: client ATA doesn't exist yet. build_and_send_refund should create it idempotently
    let refund_amount = 7_000_000u64;
    let sig = spl_tollbooth_mpp::session::build_and_send_refund(
        &rpc,
        &server_kp,
        &mint,
        6,                   // decimals
        &server_kp.pubkey(), // from_wallet (server custody)
        &client_kp.pubkey(), // to_wallet (client refund address)
        refund_amount,
    )
    .await
    .expect("refund should succeed");

    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    // Verify client received the refund
    let client_bal = rpc.get_token_account_balance(&client_ata).await.unwrap();
    assert_eq!(
        client_bal.amount, "7000000",
        "client should have received 7 tokens"
    );

    // Verify server balance decreased
    let server_bal = rpc.get_token_account_balance(&server_ata).await.unwrap();
    assert_eq!(
        server_bal.amount, "3000000",
        "server should have 3 tokens remaining"
    );

    eprintln!("session refund test passed: tx = {sig}");
}

/// Test crash recovery re-attempts a refund for a stuck Closing session.
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn crash_recovery_reattempts_refund() {
    let rpc_url = "http://127.0.0.1:8899";
    let rpc = RpcClient::new_with_commitment(rpc_url.to_string(), CommitmentConfig::confirmed());

    let server_kp = Keypair::new();
    let client_kp = Keypair::new();

    // Airdrop
    let sig = rpc
        .request_airdrop(&server_kp.pubkey(), 2 * LAMPORTS_PER_SOL)
        .await
        .expect("airdrop");
    wait(&rpc, &sig).await;

    // Create mint + server ATA with tokens
    let mint_kp = Keypair::new();
    let mint_rent = rpc
        .get_minimum_balance_for_rent_exemption(82)
        .await
        .unwrap();
    let blockhash = rpc.get_latest_blockhash().await.unwrap();
    let tx = Transaction::new_signed_with_payer(
        &[
            system_instruction::create_account(
                &server_kp.pubkey(),
                &mint_kp.pubkey(),
                mint_rent,
                82,
                &spl_token::id(),
            ),
            spl_token::instruction::initialize_mint2(
                &spl_token::id(),
                &mint_kp.pubkey(),
                &server_kp.pubkey(),
                None,
                6,
            )
            .unwrap(),
        ],
        Some(&server_kp.pubkey()),
        &[&server_kp, &mint_kp],
        blockhash,
    );
    rpc.send_and_confirm_transaction(&tx).await.unwrap();

    let mint = mint_kp.pubkey();
    let server_ata =
        spl_associated_token_account::get_associated_token_address(&server_kp.pubkey(), &mint);

    let blockhash = rpc.get_latest_blockhash().await.unwrap();
    let tx = Transaction::new_signed_with_payer(
        &[
            spl_associated_token_account::instruction::create_associated_token_account(
                &server_kp.pubkey(),
                &server_kp.pubkey(),
                &mint,
                &spl_token::id(),
            ),
            spl_token::instruction::mint_to(
                &spl_token::id(),
                &mint,
                &server_ata,
                &server_kp.pubkey(),
                &[],
                5_000_000,
            )
            .unwrap(),
        ],
        Some(&server_kp.pubkey()),
        &[&server_kp],
        blockhash,
    );
    rpc.send_and_confirm_transaction(&tx).await.unwrap();

    // Create a stuck session in the store
    let _tmpdir = tempfile::tempdir().unwrap();
    let db = libsql::Builder::new_local(_tmpdir.path().join("test.db"))
        .build()
        .await
        .unwrap();
    let store = spl_tollbooth_core::store::LibsqlStore::new(db);
    store.run_migrations().await.unwrap();

    let session = spl_tollbooth_core::types::SessionState {
        session_id: "stuck-test".into(),
        bearer_hash: "abc".into(),
        deposit_amount: 5_000_000,
        spent: 2_000_000,
        refund_address: client_kp.pubkey().to_string(),
        mint: mint.to_string(),
        decimals: 6,
        status: spl_tollbooth_core::types::SessionStatus::Closing,
        refund_signature: None, // no sig = never attempted
        created_at: 1000,
        updated_at: 1000,
    };
    store.create_session(&session).await.unwrap();

    // Run crash recovery
    spl_tollbooth_mpp::session::recover_stuck_sessions(&store, &rpc, &server_kp)
        .await
        .expect("crash recovery");

    // Session should now be Closed with a refund signature
    let recovered = store.get_session("stuck-test").await.unwrap().unwrap();
    assert_eq!(
        recovered.status,
        spl_tollbooth_core::types::SessionStatus::Closed
    );
    assert!(
        recovered.refund_signature.is_some(),
        "should have a refund signature"
    );

    // Verify client got 3 tokens (5M deposit - 2M spent = 3M refund)
    let client_ata =
        spl_associated_token_account::get_associated_token_address(&client_kp.pubkey(), &mint);
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    let bal = rpc.get_token_account_balance(&client_ata).await.unwrap();
    assert_eq!(bal.amount, "3000000", "client should get 3M token refund");

    eprintln!(
        "crash recovery test passed: refund sig = {}",
        recovered.refund_signature.unwrap()
    );
}

async fn wait(rpc: &RpcClient, sig: &Signature) {
    for _ in 0..30 {
        if let Ok(s) = rpc.get_signature_statuses(&[*sig]).await
            && s.value[0].is_some()
        {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    panic!("tx {sig} not confirmed");
}
