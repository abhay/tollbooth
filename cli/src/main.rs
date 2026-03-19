mod check;
mod init;
mod keygen;
mod serve;
mod status;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "tollbooth", about = "Tollbooth: Solana payment gateway")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the tollbooth reverse proxy server
    Serve {
        /// Path to the config file
        #[arg(long, default_value = "tollbooth.toml")]
        config: String,
    },
    /// Initialize a new tollbooth.toml config file
    Init,
    /// Validate a config file
    Check {
        /// Path to the config file
        #[arg(long, default_value = "tollbooth.toml")]
        config: String,
    },
    /// Show relayer status (pubkey and SOL balance)
    Status {
        /// Path to the config file
        #[arg(long, default_value = "tollbooth.toml")]
        config: String,
    },
    /// Generate a new Solana keypair
    Keygen {
        /// Output path for the keypair JSON file
        #[arg(long, default_value = "keypair.json")]
        output: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Serve { config } => serve::run(&config).await,
        Commands::Init => init::run(),
        Commands::Check { config } => check::run(&config),
        Commands::Status { config } => status::run(&config).await,
        Commands::Keygen { output } => keygen::run(&output),
    }
}
