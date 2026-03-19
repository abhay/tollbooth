use spl_tollbooth_core::config::TollboothConfig;

pub fn run(config_path: &str) -> anyhow::Result<()> {
    match TollboothConfig::from_file(config_path) {
        Ok(_) => {
            println!("Config OK");
            Ok(())
        }
        Err(e) => {
            anyhow::bail!("Config error: {e}");
        }
    }
}
