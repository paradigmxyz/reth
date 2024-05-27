use std::path::PathBuf;

use clap::Parser;
use eyre::Ok;
use reth_network::config::rng_secret_key;
use reth_primitives::hex::encode as hex_encode;

/// `devp2p key generate` command.
#[derive(Debug, Parser)]
pub struct Command {
    /// The path of the file to put new generated key.
    #[arg(long, value_name = "FILE", verbatim_doc_comment)]
    file: PathBuf,
}

impl Command {
    pub fn execute(&self) -> eyre::Result<()> {
        let _ = std::fs::File::create(self.file.clone())?;

        let secret = rng_secret_key();

        let hex = hex_encode(secret.as_ref());
        std::fs::write(self.file.as_path(), hex)?;

        Ok(())
    }
}
