use std::sync::Arc;

use solana_pubkey::Pubkey;
use spl_tollbooth_core::config::{ProtocolsConfig, RouteEntry, WebhooksConfig};
use spl_tollbooth_core::metrics::MetricsCollector;
use spl_tollbooth_core::store::LibsqlStore;
use spl_tollbooth_mpp::charge::MppCharge;
use spl_tollbooth_mpp::session::MppSession;
use spl_tollbooth_relayer::RelayerKind;

/// Shared application state for all handlers.
#[derive(Clone)]
pub struct AppState {
    pub store: Arc<LibsqlStore>,
    pub metrics: MetricsCollector,
    pub mpp_charge: Option<Arc<MppCharge>>,
    pub mpp_session: Option<Arc<MppSession>>,
    pub relayer: Option<Arc<RelayerKind>>,
    pub protocols: ProtocolsConfig,
    pub webhooks: Option<WebhooksConfig>,
    pub routes: Arc<Vec<RouteEntry>>,
    pub upstream: String,
    pub mint: Pubkey,
    pub decimals: u8,
    pub start_time: std::time::Instant,
    pub http_client: reqwest::Client,
}
