//! Integration test for the /relay endpoint.
//! Requires a running solana-test-validator on localhost:8899.
//!
//! Run with: cargo test -p spl-tollbooth-server --test relay_integration -- --ignored

use std::sync::Arc;

use solana_client::nonblocking::rpc_client::RpcClient;
use solana_commitment_config::CommitmentConfig;
use solana_keypair::Keypair;
use solana_native_token::LAMPORTS_PER_SOL;
use solana_signature::Signature;
use solana_signer::Signer;
use solana_system_interface::instruction as system_instruction;

/// Test the full server-first relay flow:
/// 1. Create a token mint + ATAs for payer and recipient
/// 2. Mint tokens to payer
/// 3. POST /relay/prepare to get a fee-payer-signed transaction
/// 4. Counter-sign as client, POST to /relay
/// 5. Verify the relay submits and returns a valid signature
#[tokio::test(flavor = "multi_thread")]
#[ignore] // requires solana-test-validator on localhost:8899
async fn relay_endpoint_signs_and_submits() {
    let rpc_url = "http://127.0.0.1:8899";
    let rpc = RpcClient::new_with_commitment(rpc_url.to_string(), CommitmentConfig::confirmed());

    // Generate keypairs
    let relayer_kp = Keypair::new();
    let client_kp = Keypair::new();
    let recipient_kp = Keypair::new();

    // Airdrop SOL
    let sig = rpc
        .request_airdrop(&relayer_kp.pubkey(), 2 * LAMPORTS_PER_SOL)
        .await
        .expect("airdrop relayer");
    wait_for_confirmation(&rpc, &sig).await;

    let sig = rpc
        .request_airdrop(&client_kp.pubkey(), LAMPORTS_PER_SOL)
        .await
        .expect("airdrop client");
    wait_for_confirmation(&rpc, &sig).await;

    // Create a token mint
    let mint_kp = Keypair::new();
    let mint_rent = rpc
        .get_minimum_balance_for_rent_exemption(82) // Mint account size
        .await
        .expect("get rent");

    let create_mint_ixs = vec![
        system_instruction::create_account(
            &client_kp.pubkey(),
            &mint_kp.pubkey(),
            mint_rent,
            82,
            &spl_token::id(),
        ),
        spl_token::instruction::initialize_mint2(
            &spl_token::id(),
            &mint_kp.pubkey(),
            &client_kp.pubkey(),
            None,
            6,
        )
        .expect("init mint ix"),
    ];

    let blockhash = rpc.get_latest_blockhash().await.expect("blockhash");
    let tx = solana_transaction::Transaction::new_signed_with_payer(
        &create_mint_ixs,
        Some(&client_kp.pubkey()),
        &[&client_kp, &mint_kp],
        blockhash,
    );
    let sig = rpc
        .send_and_confirm_transaction(&tx)
        .await
        .expect("create mint");
    wait_for_confirmation(&rpc, &sig).await;

    let mint = mint_kp.pubkey();

    // Create ATAs
    let client_ata =
        spl_associated_token_account::get_associated_token_address(&client_kp.pubkey(), &mint);
    let recipient_ata =
        spl_associated_token_account::get_associated_token_address(&recipient_kp.pubkey(), &mint);

    let create_atas_ixs = vec![
        spl_associated_token_account::instruction::create_associated_token_account(
            &client_kp.pubkey(),
            &client_kp.pubkey(),
            &mint,
            &spl_token::id(),
        ),
        spl_associated_token_account::instruction::create_associated_token_account(
            &client_kp.pubkey(),
            &recipient_kp.pubkey(),
            &mint,
            &spl_token::id(),
        ),
    ];

    let blockhash = rpc.get_latest_blockhash().await.expect("blockhash");
    let tx = solana_transaction::Transaction::new_signed_with_payer(
        &create_atas_ixs,
        Some(&client_kp.pubkey()),
        &[&client_kp],
        blockhash,
    );
    rpc.send_and_confirm_transaction(&tx)
        .await
        .expect("create ATAs");

    // Mint 1000 tokens to client
    let mint_to_ix = spl_token::instruction::mint_to(
        &spl_token::id(),
        &mint,
        &client_ata,
        &client_kp.pubkey(),
        &[],
        1_000_000_000,
    )
    .expect("mint_to ix");

    let blockhash = rpc.get_latest_blockhash().await.expect("blockhash");
    let tx = solana_transaction::Transaction::new_signed_with_payer(
        &[mint_to_ix],
        Some(&client_kp.pubkey()),
        &[&client_kp],
        blockhash,
    );
    rpc.send_and_confirm_transaction(&tx)
        .await
        .expect("mint to");

    // --- Start tollbooth server with relay ---
    let _tmpdir = tempfile::tempdir().expect("tempdir");
    let db = libsql::Builder::new_local(_tmpdir.path().join("test.db"))
        .build()
        .await
        .expect("build db");
    let store = Arc::new(spl_tollbooth_core::store::LibsqlStore::new(db));
    store.run_migrations().await.expect("migrations");

    let relayer = Arc::new(spl_tollbooth_relayer::RelayerKind::Builtin(
        spl_tollbooth_relayer::builtin::BuiltinRelayer::new(
            spl_tollbooth_relayer::builtin::BuiltinRelayerConfig {
                keypair: Arc::new(relayer_kp),
                rpc_url: rpc_url.to_string(),
                allowed_mints: vec![mint],
                allowed_recipients: vec![recipient_ata],
                max_transfer_amount: 10_000_000,
                requests_per_minute: 60,
                recipient: recipient_kp.pubkey(),
                decimals: 6,
            },
        )
        .expect("create relayer"),
    ));

    let state = spl_tollbooth_server::state::AppState {
        metrics: spl_tollbooth_core::metrics::MetricsCollector::new(store.clone()),
        store: store.clone(),
        mpp_charge: None,
        mpp_session: None,
        relayer: Some(relayer),
        protocols: spl_tollbooth_core::config::ProtocolsConfig { mpp: true },
        webhooks: None,
        routes: Arc::new(vec![]),
        upstream: "http://localhost:8080".into(),
        mint,
        decimals: 6,
        start_time: std::time::Instant::now(),
        http_client: reqwest::Client::new(),
    };

    let app = axum::Router::new()
        .route(
            "/relay",
            axum::routing::post(spl_tollbooth_server::relay::relay_handler),
        )
        .route(
            "/relay/prepare",
            axum::routing::post(spl_tollbooth_server::relay::prepare_handler),
        )
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");

    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });

    // --- Prepare → counter-sign → submit flow ---
    let http = reqwest::Client::new();

    // 1. Prepare: server builds and fee-payer-signs the transaction
    let resp = http
        .post(format!("http://{addr}/relay/prepare"))
        .json(&serde_json::json!({
            "payer": client_kp.pubkey().to_string(),
            "amount": "1",
        }))
        .send()
        .await
        .expect("prepare request");
    assert_eq!(resp.status().as_u16(), 200, "prepare should return 200");
    let tx_bytes = resp.bytes().await.expect("prepare response bytes");

    // 2. Deserialize and counter-sign as client (2nd signer, index 1)
    let mut tx: solana_transaction::versioned::VersionedTransaction =
        bincode::deserialize(&tx_bytes).expect("deserialize prepared tx");
    let message_bytes = tx.message.serialize();
    let client_sig = client_kp.sign_message(&message_bytes);
    tx.signatures[1] = client_sig;

    // 3. Submit the fully-signed transaction
    let signed_bytes = bincode::serialize(&tx).expect("serialize signed tx");
    let resp = http
        .post(format!("http://{addr}/relay"))
        .header("Content-Type", "application/octet-stream")
        .body(signed_bytes)
        .send()
        .await
        .expect("relay request");

    let status = resp.status();
    let body: serde_json::Value = resp.json().await.expect("parse response");

    assert_eq!(
        status.as_u16(),
        200,
        "relay should return 200, got {status}: {body}"
    );

    let sig_str = body["signature"].as_str().expect("signature is string");
    assert!(!sig_str.is_empty(), "signature should not be empty");

    // Verify the transaction landed on-chain
    let sig: Signature = sig_str.parse().expect("parse signature");
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    let recipient_balance = rpc
        .get_token_account_balance(&recipient_ata)
        .await
        .expect("get recipient balance");
    assert_eq!(
        recipient_balance.amount, "1000000",
        "recipient should have received 1000000 raw tokens"
    );

    eprintln!("relay test passed: tx signature = {sig}");
}

async fn wait_for_confirmation(rpc: &RpcClient, sig: &Signature) {
    for _ in 0..30 {
        if let Ok(statuses) = rpc.get_signature_statuses(&[*sig]).await
            && statuses.value[0].is_some()
        {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    panic!("transaction {sig} not confirmed after 15s");
}
