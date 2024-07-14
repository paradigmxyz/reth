use alloy_primitives::{hex, B256};
use clap::Parser;
use k256::ecdsa::SigningKey;
use reth_tracing::tracing::info;

#[derive(Debug, Parser)]
pub struct Command {
    view: B256,
    spend: B256,
}

impl Command {
    pub async fn execute(self) -> eyre::Result<()> {
        info!(target: "reth::cli", "Generating stealth meta address");

        let view = SigningKey::from_slice(self.view.as_slice())?;
        let view = hex::encode(view.verifying_key().to_encoded_point(true).as_bytes());

        let spend = SigningKey::from_slice(self.spend.as_slice())?;
        let spend = hex::encode(spend.verifying_key().to_encoded_point(true).as_bytes());

        let meta = format!("sth:eth:{view}{spend}");

        info!(meta);

        Ok(())
    }
}
