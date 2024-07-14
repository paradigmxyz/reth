use crate::crypto;
use alloy_primitives::{hex, B256};
use clap::Parser;
use k256::ecdsa::{SigningKey, VerifyingKey};
use reth_tracing::tracing::info;

#[derive(Debug, Clone)]
pub struct StealthMeta {
    /// View public key
    pub view_pub: VerifyingKey,
    /// Spend public key
    pub spend_pub: VerifyingKey,
}

#[derive(Debug, Parser)]
pub struct Command {
    #[arg(value_parser = parse_stealth_meta_address)]
    recipient: StealthMeta,
    note: Option<String>,
}

impl Command {
    pub async fn execute(self) -> eyre::Result<()> {
        info!(target: "reth::cli", "Generating random stealth address");

        let ephemeral = SigningKey::from_slice(B256::random().as_slice())?;

        let (stealth_address, view_tag) = crypto::stealth_address(
            &self.recipient.spend_pub,
            &self.recipient.view_pub,
            &ephemeral,
        );

        let eph_pub =
            hex::encode_prefixed(ephemeral.verifying_key().to_encoded_point(true).as_bytes());

        info!("🔐 🔐 🔐");
        info!(
            ephemeral_public_key = ?eph_pub,
            ?view_tag,
            ?stealth_address,
        );

        if let Some(note) = self.note {
            let mut payload = vec![view_tag];
            payload.extend(
                crypto::try_encrypt_note(&ephemeral, &self.recipient.view_pub, &note).unwrap(),
            );
            info!("encrypted note with view tag prefix:\"{}\"", hex::encode_prefixed(payload))
        }
        info!("🔐 🔐 🔐");

        Ok(())
    }
}

fn parse_stealth_meta_address(value: &str) -> eyre::Result<StealthMeta> {
    let expect_str = "right meta format.";

    let public_keys = value.split(':').last().expect(expect_str);
    let public_keys = hex::decode(public_keys).expect(expect_str);
    assert!(public_keys.len() == 66, "{expect_str}");

    Ok(StealthMeta {
        view_pub: crypto::to_verifying_key(&public_keys[..33]).unwrap(),
        spend_pub: crypto::to_verifying_key(&public_keys[33..]).unwrap(),
    })
}
