//! End-to-end integration tests for the full proxy payment flow.
//! Requires a running solana-test-validator on localhost:8899.
//!
//! Run with: cargo test -p spl-tollbooth-server --test e2e_proxy -- --ignored
//!
//! Tests run in parallel; each uses fresh keypairs so no state conflicts.

use std::sync::Arc;

use solana_client::nonblocking::rpc_client::RpcClient;
use solana_commitment_config::CommitmentConfig;
use solana_keypair::Keypair;
use solana_native_token::LAMPORTS_PER_SOL;
use solana_signature::Signature;
use solana_signer::Signer;
use solana_system_interface::instruction as system_instruction;

const RPC_URL: &str = "http://127.0.0.1:8899";

// ============================================================
// Test: Full MPP charge flow through proxy
// ============================================================

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn e2e_mpp_charge_flow() {
    let rpc = new_rpc();
    let (relayer_kp, client_kp, recipient_kp, mint, _client_ata, recipient_ata) =
        setup_token_env(&rpc).await;

    let (base, _handle, _tmpdir) =
        start_proxy(&rpc, &relayer_kp, &recipient_kp, mint, recipient_ata).await;

    let http = reqwest::Client::new();

    // 1. Unpaid → 402 with MPP challenge
    let resp = http
        .get(format!("{base}/api/joke"))
        .header("X-Payment-Protocol", "mpp")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 402);
    let challenge: serde_json::Value = resp.json().await.unwrap();
    assert!(challenge.get("amount").is_some());
    assert!(challenge.get("recipient").is_some());
    assert!(challenge.get("fee_payer").is_some());
    assert!(challenge.get("relay_url").is_some());

    // 2. Relay payment via prepare → counter-sign → submit
    let mpp_sig = relay_payment(&http, &base, &client_kp, "1").await;

    // 3. Paid request → 200 with upstream body + headers
    let resp = http
        .get(format!("{base}/api/joke"))
        .header("X-Payment-Protocol", "mpp")
        .header("X-Payment-Signature", &mpp_sig)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(resp.headers().get("x-tollbooth-verified").unwrap(), "true");
    assert_eq!(resp.headers().get("x-tollbooth-protocol").unwrap(), "mpp");
    assert_eq!(
        resp.headers().get("x-tollbooth-event").unwrap(),
        "payment.completed"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["joke"], "test-joke-from-upstream");

    // 4. Replay → 200 (idempotent: returns cached receipt)
    let resp = http
        .get(format!("{base}/api/joke"))
        .header("X-Payment-Protocol", "mpp")
        .header("X-Payment-Signature", &mpp_sig)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status().as_u16(),
        200,
        "replay should return cached receipt (idempotent)"
    );
    assert_eq!(resp.headers().get("x-tollbooth-verified").unwrap(), "true");

    // 5. On-chain: recipient got tokens
    let bal = rpc.get_token_account_balance(&recipient_ata).await.unwrap();
    assert_eq!(bal.amount, "1000000");
}

// ============================================================
// Test: Unmatched routes pass through without payment gating
// ============================================================

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn e2e_unmatched_route_passes_through() {
    let rpc = new_rpc();
    let (_relayer_kp, _client_kp, _recipient_kp, mint, _client_ata, _recipient_ata) =
        setup_token_env(&rpc).await;

    // Upstream serves /health (not in route config)
    let upstream_app = axum::Router::new().route("/health", axum::routing::get(|| async { "ok" }));
    let upstream_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_addr = upstream_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(upstream_listener, upstream_app).await.ok();
    });

    let _dir = tempfile::tempdir().unwrap();
    let db = libsql::Builder::new_local(_dir.path().join("test.db"))
        .build()
        .await
        .unwrap();
    let store = Arc::new(spl_tollbooth_core::store::LibsqlStore::new(db));
    store.run_migrations().await.unwrap();

    let state = make_state(store, None, None, None, &upstream_addr.to_string(), mint);
    let router = spl_tollbooth_server::proxy::ProxyServer {
        config: make_config(),
        state,
    }
    .router();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.ok();
    });

    // /health is not in routes → passes through
    let resp = reqwest::get(format!("http://{addr}/health")).await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(resp.text().await.unwrap(), "ok");
}

// ============================================================
// Helpers
// ============================================================

fn new_rpc() -> RpcClient {
    RpcClient::new_with_commitment(RPC_URL.to_string(), CommitmentConfig::confirmed())
}

/// Set up a fresh token environment: mint, ATAs, fund client with 10 tokens.
async fn setup_token_env(
    rpc: &RpcClient,
) -> (
    Keypair,
    Keypair,
    Keypair,
    solana_pubkey::Pubkey,
    solana_pubkey::Pubkey,
    solana_pubkey::Pubkey,
) {
    let relayer_kp = Keypair::new();
    let client_kp = Keypair::new();
    let recipient_kp = Keypair::new();

    let sig = rpc
        .request_airdrop(&relayer_kp.pubkey(), 2 * LAMPORTS_PER_SOL)
        .await
        .unwrap();
    wait(rpc, &sig).await;
    let sig = rpc
        .request_airdrop(&client_kp.pubkey(), 2 * LAMPORTS_PER_SOL)
        .await
        .unwrap();
    wait(rpc, &sig).await;

    let mint_kp = Keypair::new();
    let mint_rent = rpc
        .get_minimum_balance_for_rent_exemption(82)
        .await
        .unwrap();
    let blockhash = rpc.get_latest_blockhash().await.unwrap();
    let tx = solana_transaction::Transaction::new_signed_with_payer(
        &[
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
            .unwrap(),
        ],
        Some(&client_kp.pubkey()),
        &[&client_kp, &mint_kp],
        blockhash,
    );
    rpc.send_and_confirm_transaction(&tx).await.unwrap();

    let mint = mint_kp.pubkey();
    let client_ata =
        spl_associated_token_account::get_associated_token_address(&client_kp.pubkey(), &mint);
    let recipient_ata =
        spl_associated_token_account::get_associated_token_address(&recipient_kp.pubkey(), &mint);

    let blockhash = rpc.get_latest_blockhash().await.unwrap();
    let tx = solana_transaction::Transaction::new_signed_with_payer(
        &[
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
            spl_token::instruction::mint_to(
                &spl_token::id(),
                &mint,
                &client_ata,
                &client_kp.pubkey(),
                &[],
                10_000_000,
            )
            .unwrap(),
        ],
        Some(&client_kp.pubkey()),
        &[&client_kp],
        blockhash,
    );
    rpc.send_and_confirm_transaction(&tx).await.unwrap();

    (
        relayer_kp,
        client_kp,
        recipient_kp,
        mint,
        client_ata,
        recipient_ata,
    )
}

/// Relay an MPP payment using the server-first flow:
/// 1. POST /relay/prepare with payer + amount → get partially-signed tx
/// 2. Counter-sign as client
/// 3. POST /relay with fully-signed tx → get signature
async fn relay_payment(
    http: &reqwest::Client,
    base: &str,
    client_kp: &Keypair,
    amount: &str,
) -> String {
    // 1. Prepare: server builds and fee-payer-signs the transaction
    let resp = http
        .post(format!("{base}/relay/prepare"))
        .json(&serde_json::json!({
            "payer": client_kp.pubkey().to_string(),
            "amount": amount,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200, "prepare should succeed");
    let tx_bytes = resp.bytes().await.unwrap();

    // 2. Deserialize and counter-sign as client (2nd signer, index 1)
    let mut tx: solana_transaction::versioned::VersionedTransaction =
        bincode::deserialize(&tx_bytes).unwrap();
    let message_bytes = tx.message.serialize();
    let client_sig = client_kp.sign_message(&message_bytes);
    tx.signatures[1] = client_sig;

    // 3. Submit the fully-signed transaction
    let signed_bytes = bincode::serialize(&tx).unwrap();
    let resp = http
        .post(format!("{base}/relay"))
        .header("Content-Type", "application/octet-stream")
        .body(signed_bytes)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200, "relay should succeed");
    let body: serde_json::Value = resp.json().await.unwrap();
    let sig = body["signature"].as_str().unwrap().to_string();

    let rpc = new_rpc();
    wait(&rpc, &sig.parse().unwrap()).await;
    sig
}

/// Start a tollbooth proxy with a mock upstream, returns (base_url, join_handle).
async fn start_proxy(
    _rpc: &RpcClient,
    relayer_kp: &Keypair,
    recipient_kp: &Keypair,
    mint: solana_pubkey::Pubkey,
    recipient_ata: solana_pubkey::Pubkey,
) -> (String, tokio::task::JoinHandle<()>, tempfile::TempDir) {
    // Mock upstream
    let upstream_app = axum::Router::new().route(
        "/api/joke",
        axum::routing::get(|| async {
            axum::Json(serde_json::json!({"joke": "test-joke-from-upstream"}))
        }),
    );
    let upstream_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_addr = upstream_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(upstream_listener, upstream_app).await.ok();
    });

    let tmp_dir = tempfile::tempdir().unwrap();
    let db = libsql::Builder::new_local(tmp_dir.path().join("test.db"))
        .build()
        .await
        .unwrap();
    let store = Arc::new(spl_tollbooth_core::store::LibsqlStore::new(db));
    store.run_migrations().await.unwrap();

    let shared_rpc = Arc::new(RpcClient::new_with_commitment(
        RPC_URL.to_string(),
        CommitmentConfig::confirmed(),
    ));

    let relayer = Arc::new(spl_tollbooth_relayer::RelayerKind::Builtin(
        spl_tollbooth_relayer::builtin::BuiltinRelayer::new(
            spl_tollbooth_relayer::builtin::BuiltinRelayerConfig {
                keypair: Arc::new(Keypair::new_from_array(
                    relayer_kp.to_bytes()[..32].try_into().unwrap(),
                )),
                rpc_url: RPC_URL.to_string(),
                allowed_mints: vec![mint],
                allowed_recipients: vec![recipient_ata],
                max_transfer_amount: 10_000_000,
                requests_per_minute: 120,
                recipient: recipient_kp.pubkey(),
                decimals: 6,
            },
        )
        .unwrap(),
    ));

    let mpp_charge = Some(Arc::new(spl_tollbooth_mpp::charge::MppCharge {
        ctx: spl_tollbooth_mpp::context::MppContext {
            recipient: recipient_kp.pubkey(),
            mint,
            decimals: 6,
            rpc_client: shared_rpc.clone(),
            store: store.clone(),
            relayer_pubkey: Some(relayer_kp.pubkey()),
            relay_url: "http://localhost:0/relay".into(),
            platform_fee_recipient: None,
            platform_fee_flat_raw: 0,
            platform_fee_percent: 0.0,
        },
    }));

    let state = make_state(
        store,
        Some(relayer),
        mpp_charge,
        None,
        &upstream_addr.to_string(),
        mint,
    );

    let router = spl_tollbooth_server::proxy::ProxyServer {
        config: make_config(),
        state,
    }
    .router();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        axum::serve(listener, router).await.ok();
    });

    (format!("http://{addr}"), handle, tmp_dir)
}

fn make_state(
    store: Arc<spl_tollbooth_core::store::LibsqlStore>,
    relayer: Option<Arc<spl_tollbooth_relayer::RelayerKind>>,
    mpp_charge: Option<Arc<spl_tollbooth_mpp::charge::MppCharge>>,
    mpp_session: Option<Arc<spl_tollbooth_mpp::session::MppSession>>,
    upstream: &str,
    mint: solana_pubkey::Pubkey,
) -> spl_tollbooth_server::state::AppState {
    spl_tollbooth_server::state::AppState {
        metrics: spl_tollbooth_core::metrics::MetricsCollector::new(store.clone()),
        store,
        mpp_charge,
        mpp_session,
        relayer,
        protocols: spl_tollbooth_core::config::ProtocolsConfig { mpp: true },
        webhooks: None,
        routes: Arc::new(vec![spl_tollbooth_core::config::RouteEntry {
            path: "/api/joke".into(),
            method: Some("GET".into()),
            price: "1".into(),
            mode: spl_tollbooth_core::config::RouteMode::Charge,
            deposit: None,
        }]),
        upstream: format!("http://{upstream}"),
        mint,
        decimals: 6,
        start_time: std::time::Instant::now(),
        http_client: reqwest::Client::new(),
    }
}

fn make_config() -> spl_tollbooth_core::config::TollboothConfig {
    spl_tollbooth_core::config::TollboothConfig {
        server: spl_tollbooth_core::config::ServerConfig {
            listen: "0.0.0.0:0".into(),
            upstream: Some("unused".into()),
            relay_url: None,
        },
        solana: spl_tollbooth_core::config::SolanaConfig {
            network: "localnet".into(),
            rpc_url: RPC_URL.into(),
            recipient: "unused".into(),
            mint: "unused".into(),
            decimals: 6,
            keypair_path: "unused".into(),
        },
        relayer: spl_tollbooth_core::config::RelayerConfig {
            mode: spl_tollbooth_core::config::RelayerMode::Builtin,
            endpoint: None,
            api_key: None,
            max_transfer_amount: Some(10_000_000),
        },
        database: spl_tollbooth_core::config::DatabaseConfig {
            url: ":memory:".into(),
            token: None,
        },
        protocols: spl_tollbooth_core::config::ProtocolsConfig { mpp: true },
        routes: vec![],
        webhooks: None,
        logging: None,
        platform_fee: None,
    }
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
