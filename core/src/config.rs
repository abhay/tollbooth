use serde::Deserialize;

use crate::error::ConfigError;

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum RouteMode {
    Charge,
    Session,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum RelayerMode {
    Builtin,
    External,
    Disabled,
}

#[derive(Debug, Deserialize)]
pub struct TollboothConfig {
    pub server: ServerConfig,
    pub solana: SolanaConfig,
    pub relayer: RelayerConfig,
    pub database: DatabaseConfig,
    pub protocols: ProtocolsConfig,
    #[serde(default)]
    pub routes: Vec<RouteEntry>,
    pub webhooks: Option<WebhooksConfig>,
    pub logging: Option<LoggingConfig>,
}

#[derive(Debug, Deserialize)]
pub struct ServerConfig {
    pub listen: String,
    pub upstream: Option<String>,
    /// Public URL for the /relay endpoint (e.g. "https://pay.example.com/relay").
    /// If omitted, derived from `listen` as `http://{listen}/relay`.
    pub relay_url: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SolanaConfig {
    pub network: String,
    pub rpc_url: String,
    pub recipient: String,
    pub mint: String,
    pub decimals: u8,
    pub keypair_path: String,
}

#[derive(Debug, Deserialize)]
pub struct RelayerConfig {
    pub mode: RelayerMode,
    pub endpoint: Option<String>,
    pub api_key: Option<String>,
    pub max_transfer_amount: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct DatabaseConfig {
    pub url: String,
    pub token: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProtocolsConfig {
    pub mpp: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RouteEntry {
    pub path: String,
    pub method: Option<String>,
    pub price: String,
    pub mode: RouteMode,
    pub deposit: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WebhooksConfig {
    pub enabled: bool,
    pub url: Option<String>,
    pub secret: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct LoggingConfig {
    pub level: Option<String>,
    pub format: Option<String>,
}

impl TollboothConfig {
    pub fn from_file(path: &str) -> Result<Self, ConfigError> {
        let content = std::fs::read_to_string(path)?;
        let config: Self =
            toml::from_str(&content).map_err(|e| ConfigError::Parse(e.to_string()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.server.upstream.is_none() {
            return Err(ConfigError::Validation(
                "server.upstream is required for proxy mode".into(),
            ));
        }
        if !["mainnet-beta", "devnet", "testnet", "localnet"]
            .contains(&self.solana.network.as_str())
        {
            return Err(ConfigError::Validation(format!(
                "invalid network: {}",
                self.solana.network
            )));
        }
        if self.relayer.mode == RelayerMode::External && self.relayer.endpoint.is_none() {
            return Err(ConfigError::Validation(
                "relayer.endpoint required for external mode".into(),
            ));
        }
        for route in &self.routes {
            if route.mode == RouteMode::Session && route.deposit.is_none() {
                return Err(ConfigError::Validation(format!(
                    "session route {} requires deposit",
                    route.path
                )));
            }
        }
        if let Some(ref wh) = self.webhooks
            && wh.enabled
            && wh.secret.as_ref().is_none_or(|s| s.is_empty())
        {
            return Err(ConfigError::Validation(
                "webhooks.secret is required when webhooks are enabled".into(),
            ));
        }
        Ok(())
    }
}
