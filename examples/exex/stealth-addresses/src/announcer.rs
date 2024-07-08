#![allow(dead_code)]

use crate::crypto;
use alloy_sol_types::{sol, SolEvent};
use k256::ecdsa::SigningKey;
use reth::primitives::{address, Address, B256, U256};
use reth_execution_types::Chain;
use reth_tracing::tracing::info;
use std::str::FromStr;

const ANNOUNCER: Address = address!("55649E01B5Df198D18D95b5cc5051630cfD45564");

sol!(
    event ERC5564Announcement(
        uint256 indexed schemeId,
        address indexed stealthAddress,
        address indexed caller,
        bytes ephemeralPubKey,
        bytes metadata
    );
);

fn access_keystore() -> SigningKey {
    let view = B256::from_str(&std::env::var("VIEW_KEY").expect("exists")).expect("valid");
    SigningKey::from_slice(view.as_slice()).expect("valid")
}

/// Checks the blocks for any stealth address announcements according to [ERC-5564](https://eips.ethereum.org/EIPS/eip-5564).
pub(crate) fn peek(chain: &Chain) {
    let view = access_keystore();
    for announcement in get_announcements(chain) {
        if let Some(ephemeral_pub) = crypto::to_verifying_key(&announcement.ephemeralPubKey) {
            let view_tag = announcement.metadata[0];
            if crypto::is_ours(&view, &ephemeral_pub, view_tag) {
                info!(
                    ?ephemeral_pub,
                    announcer = ?announcement.caller,
                    stealth_address = ?announcement.stealthAddress,
                    "🎉 One of us! One of us! 🎉"
                );

                if let Some(note) =
                    crypto::try_decrypt_node(&view, &ephemeral_pub, &announcement.metadata[1..])
                {
                    info!("🔐 Found a secure note! 🔐\n{note}");
                }
            }
        }
    }
}

/// Gets all [ERC-5564](https://eips.ethereum.org/EIPS/eip-5564) announcements with `secp256k1` scheme.
fn get_announcements(chain: &Chain) -> Vec<ERC5564Announcement> {
    chain
        .block_receipts_iter()
        .flat_map(|receipts| receipts.iter().flatten())
        .flat_map(|r| r.logs.iter())
        .filter(|l| l.address == ANNOUNCER)
        .filter_map(|l| ERC5564Announcement::decode_log(l, false).ok().map(|e| e.data))
        .filter(|ev| ev.schemeId == U256::from(1) && !ev.metadata.is_empty())
        .collect()
}
