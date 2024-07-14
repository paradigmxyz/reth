use alloy_primitives::{hex, Address, B256};
use clap::Parser;
use k256::ecdsa::{SigningKey, VerifyingKey};
use reth_tracing::tracing::info;

use crate::crypto;

#[derive(Debug, Parser)]
pub struct Command {
    #[arg(value_parser = parse_ephemeral_public_key)]
    ephemeral: VerifyingKey,
    view: B256,
    spend: B256,
}

impl Command {
    pub async fn execute(self) -> eyre::Result<()> {
        info!(target: "reth::cli", "Generating stealth address private key");

        let view = SigningKey::from_slice(self.view.as_slice())?;
        let spend = SigningKey::from_slice(self.spend.as_slice())?;

        let stealth_key = crypto::stealth_key(&spend, &view, &self.ephemeral, None).unwrap();

        let stealth_address = Address::from_private_key(&stealth_key);
        let stealth_key = hex::encode_prefixed(stealth_key.to_bytes());

        info!(?stealth_address, stealth_key);

        Ok(())
    }
}

fn parse_ephemeral_public_key(value: &str) -> eyre::Result<VerifyingKey> {
    let expect_str = "to be 33 bytes.";

    let eph_pub = hex::decode(value).expect(expect_str);
    assert!(eph_pub.len() == 33, "{expect_str}");

    Ok(crypto::to_verifying_key(&eph_pub).unwrap())
}
