use std::sync::Arc;

use solana_client::nonblocking::rpc_client::RpcClient;
use solana_pubkey::Pubkey;
use spl_tollbooth_core::store::LibsqlStore;

/// Shared context for MPP protocol handlers (charge and session).
pub struct MppContext {
    pub recipient: Pubkey,
    pub mint: Pubkey,
    pub decimals: u8,
    pub rpc_client: Arc<RpcClient>,
    pub store: Arc<LibsqlStore>,
    pub relayer_pubkey: Option<Pubkey>,
    pub relay_url: String,
}
