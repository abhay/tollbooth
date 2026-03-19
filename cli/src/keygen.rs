use std::path::Path;

use solana_keypair::Keypair;
use solana_signer::Signer;

pub fn run(output_path: &str) -> anyhow::Result<()> {
    let path = Path::new(output_path);
    if path.exists() {
        anyhow::bail!("{output_path} already exists, refusing to overwrite");
    }

    let keypair = Keypair::new();
    let bytes = keypair.to_bytes();
    let json = serde_json::to_string(&bytes.to_vec())?;

    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(json.as_bytes())?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, &json)?;
    }
    println!("Wrote new keypair to {output_path}");
    println!("Public key: {}", keypair.pubkey());

    Ok(())
}
