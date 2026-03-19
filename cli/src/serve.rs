use std::sync::Arc;

use solana_client::nonblocking::rpc_client::RpcClient;
use solana_commitment_config::CommitmentConfig;
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use spl_tollbooth_core::config::TollboothConfig;
use spl_tollbooth_core::store::LibsqlStore;
use spl_tollbooth_core::webhooks;
use spl_tollbooth_mpp::charge::MppCharge;
use spl_tollbooth_mpp::context::MppContext;
use spl_tollbooth_mpp::session::{self, MppSession};
use spl_tollbooth_relayer::RelayerKind;
use spl_tollbooth_relayer::builtin::{BuiltinRelayer, BuiltinRelayerConfig};
use spl_tollbooth_relayer::disabled::DisabledRelayer;
use spl_tollbooth_relayer::external::{ExternalRelayer, ExternalRelayerConfig};
use spl_tollbooth_server::proxy::ProxyServer;
use spl_tollbooth_server::state::AppState;
use tracing_subscriber::EnvFilter;

pub async fn run(config_path: &str) -> anyhow::Result<()> {
    let config = TollboothConfig::from_file(config_path)
        .map_err(|e| anyhow::anyhow!("failed to load config: {e}"))?;

    // Set up tracing
    let log_level = config
        .logging
        .as_ref()
        .and_then(|l| l.level.as_deref())
        .unwrap_or("info");
    let log_format = config
        .logging
        .as_ref()
        .and_then(|l| l.format.as_deref())
        .unwrap_or("pretty");

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(log_level));

    match log_format {
        "json" => {
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .json()
                .init();
        }
        _ => {
            tracing_subscriber::fmt().with_env_filter(filter).init();
        }
    }

    // Init store — store owns the Database handle and creates fresh connections per operation
    let store = if let Some(ref token) = config.database.token {
        let db = libsql::Builder::new_remote(config.database.url.clone(), token.to_string())
            .build()
            .await?;
        LibsqlStore::new(db)
    } else {
        let db = libsql::Builder::new_local(&config.database.url)
            .build()
            .await?;
        LibsqlStore::new(db)
    };
    store.run_migrations().await?;
    let store = Arc::new(store);

    // Load keypair
    let keypair_bytes: Vec<u8> = {
        let raw = std::fs::read_to_string(&config.solana.keypair_path)
            .map_err(|e| anyhow::anyhow!("failed to read keypair: {e}"))?;
        serde_json::from_str(&raw)
            .map_err(|e| anyhow::anyhow!("failed to parse keypair JSON: {e}"))?
    };
    let keypair = Keypair::try_from(keypair_bytes.as_slice())
        .map_err(|e| anyhow::anyhow!("invalid keypair: {e}"))?;
    let keypair = Arc::new(keypair);

    // Create RPC client
    let rpc_client = Arc::new(RpcClient::new_with_commitment(
        config.solana.rpc_url.clone(),
        CommitmentConfig::confirmed(),
    ));

    // Parse common pubkeys
    let recipient: Pubkey = config
        .solana
        .recipient
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid recipient pubkey: {e}"))?;
    let mint: Pubkey = config
        .solana
        .mint
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid mint pubkey: {e}"))?;
    let decimals = config.solana.decimals;

    // Fail fast if the configured mint is a Token-2022 mint (not yet supported).
    let mint_account = rpc_client
        .get_account(&mint)
        .await
        .map_err(|e| anyhow::anyhow!("failed to fetch mint account {mint}: {e}"))?;
    let token_2022_program_id: Pubkey = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb"
        .parse()
        .unwrap();
    if mint_account.owner == token_2022_program_id {
        anyhow::bail!(
            "mint {mint} is a Token-2022 mint, which is not yet supported. \
             The relayer and transaction validator only handle classic SPL Token mints."
        );
    }

    // Build relayer
    let relayer: Arc<RelayerKind> = match config.relayer.mode {
        spl_tollbooth_core::config::RelayerMode::Builtin => {
            let r = BuiltinRelayer::new(BuiltinRelayerConfig {
                keypair: keypair.clone(),
                rpc_url: config.solana.rpc_url.clone(),
                allowed_mints: vec![mint],
                allowed_recipients: vec![recipient],
                max_transfer_amount: config.relayer.max_transfer_amount.unwrap_or(10_000),
                requests_per_minute: 60,
                recipient,
                decimals,
            })?;
            Arc::new(RelayerKind::Builtin(r))
        }
        spl_tollbooth_core::config::RelayerMode::External => {
            let endpoint =
                config.relayer.endpoint.clone().ok_or_else(|| {
                    anyhow::anyhow!("relayer.endpoint required for external mode")
                })?;
            let fee_payer_pubkey = keypair.pubkey(); // use server keypair as fee payer identity
            let r = ExternalRelayer::new(ExternalRelayerConfig {
                endpoint,
                api_key: config.relayer.api_key.clone(),
                fee_payer_pubkey,
            })?;
            Arc::new(RelayerKind::External(r))
        }
        spl_tollbooth_core::config::RelayerMode::Disabled => {
            Arc::new(RelayerKind::Disabled(DisabledRelayer))
        }
    };

    let relayer_pubkey = if relayer.is_disabled() {
        None
    } else {
        Some(relayer.fee_payer())
    };
    let relay_url = if config.relayer.mode != spl_tollbooth_core::config::RelayerMode::Disabled {
        Some(
            config
                .server
                .relay_url
                .clone()
                .unwrap_or_else(|| format!("http://{}/relay", config.server.listen)),
        )
    } else {
        None
    };

    // Build protocol handlers
    let default_relay = || {
        relay_url
            .clone()
            .unwrap_or_else(|| format!("http://{}/relay", config.server.listen))
    };

    let mpp_charge = if config.protocols.mpp {
        Some(Arc::new(MppCharge {
            ctx: MppContext {
                recipient,
                mint,
                decimals,
                rpc_client: rpc_client.clone(),
                store: store.clone(),
                relayer_pubkey,
                relay_url: default_relay(),
            },
        }))
    } else {
        None
    };

    let mpp_session = if config.protocols.mpp {
        Some(Arc::new(MppSession::new(
            MppContext {
                recipient,
                mint,
                decimals,
                rpc_client: rpc_client.clone(),
                store: store.clone(),
                relayer_pubkey,
                relay_url: default_relay(),
            },
            keypair.clone(),
        )))
    } else {
        None
    };

    // Crash recovery for stuck sessions
    if config.protocols.mpp {
        tracing::info!("running crash recovery for stuck sessions");
        if let Err(e) = session::recover_stuck_sessions(&store, &rpc_client, &keypair).await {
            tracing::warn!("crash recovery encountered errors: {e}");
        }
    }

    // Start webhook delivery background task
    if let Some(ref wh_config) = config.webhooks
        && wh_config.enabled
    {
        tracing::info!("starting webhook delivery task");
        webhooks::spawn_webhook_task(store.clone(), wh_config.clone());
    }

    // Start metrics rollup background task
    spl_tollbooth_core::metrics::spawn_rollup_task(store.clone());

    // Create batching metrics collector
    let metrics = spl_tollbooth_core::metrics::MetricsCollector::new(store.clone());

    let upstream = config
        .server
        .upstream
        .clone()
        .unwrap_or_else(|| "http://localhost:8080".into());

    // Build app state
    let state = AppState {
        store,
        metrics,
        mpp_charge,
        mpp_session,
        relayer: Some(relayer),
        protocols: config.protocols.clone(),
        webhooks: config.webhooks.clone(),
        routes: Arc::new(config.routes.clone()),
        upstream,
        mint,
        decimals,
        start_time: std::time::Instant::now(),
        http_client: reqwest::Client::new(),
    };

    let server = ProxyServer::from_config(config, state).await;
    server.run().await?;

    Ok(())
}
