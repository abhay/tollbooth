use solana_client::nonblocking::rpc_client::RpcClient;
use solana_commitment_config::CommitmentConfig;
use solana_keypair::Keypair;
use solana_signer::Signer;
use spl_tollbooth_core::config::TollboothConfig;
use std::convert::TryFrom;

pub async fn run(config_path: &str) -> anyhow::Result<()> {
    let config = TollboothConfig::from_file(config_path)
        .map_err(|e| anyhow::anyhow!("failed to load config: {e}"))?;

    // Load keypair
    let keypair_bytes: Vec<u8> = {
        let raw = std::fs::read_to_string(&config.solana.keypair_path)
            .map_err(|e| anyhow::anyhow!("failed to read keypair: {e}"))?;
        serde_json::from_str(&raw)
            .map_err(|e| anyhow::anyhow!("failed to parse keypair JSON: {e}"))?
    };
    let keypair = Keypair::try_from(keypair_bytes.as_slice())
        .map_err(|e| anyhow::anyhow!("invalid keypair: {e}"))?;

    let pubkey = keypair.pubkey();
    println!("Relayer pubkey: {pubkey}");

    // Connect to RPC and fetch balance
    let rpc_client = RpcClient::new_with_commitment(
        config.solana.rpc_url.clone(),
        CommitmentConfig::confirmed(),
    );

    match rpc_client.get_balance(&pubkey).await {
        Ok(lamports) => {
            let sol = lamports as f64 / 1_000_000_000.0;
            println!("SOL balance:    {sol:.9} SOL ({lamports} lamports)");
        }
        Err(e) => {
            println!("SOL balance:    (error: {e})");
        }
    }

    Ok(())
}
