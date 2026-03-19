use spl_tollbooth_core::config::TollboothConfig;

#[test]
fn parse_example_config() {
    let toml_str = include_str!("../../tollbooth.example.toml");
    let config: TollboothConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.server.listen, "0.0.0.0:3402");
    assert_eq!(config.solana.network, "mainnet-beta");
    assert!(config.protocols.mpp);
    assert_eq!(config.routes.len(), 2);
}

#[test]
fn reject_missing_upstream() {
    let toml_str = r#"
[server]
listen = "0.0.0.0:3402"

[solana]
network = "devnet"
rpc_url = "https://api.devnet.solana.com"
recipient = "11111111111111111111111111111111"
mint = "11111111111111111111111111111111"
decimals = 6
keypair_path = "./keypair.json"

[relayer]
mode = "disabled"

[database]
url = "file:test.db"

[protocols]
mpp = true
"#;
    let config: TollboothConfig = toml::from_str(toml_str).unwrap();
    assert!(config.validate().is_err());
}
