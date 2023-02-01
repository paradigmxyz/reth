#![allow(unused)]
//! Test helper impls
use reth_eth_wire::BlockBody;
use reth_interfaces::test_utils::generators::random_block_range;
use reth_primitives::{SealedHeader, H256};
use std::collections::HashMap;

/// Metrics scope used for testing.
pub(crate) const TEST_SCOPE: &str = "downloaders.test";

/// Generate a set of bodies and their corresponding block hashes
pub(crate) fn generate_bodies(
    rng: std::ops::Range<u64>,
) -> (Vec<SealedHeader>, HashMap<H256, BlockBody>) {
    let blocks = random_block_range(rng, H256::zero(), 0..2);

    let headers = blocks.iter().map(|block| block.header.clone()).collect();
    let bodies = blocks
        .into_iter()
        .map(|block| {
            (
                block.hash(),
                BlockBody {
                    transactions: block.body,
                    ommers: block.ommers.into_iter().map(|header| header.unseal()).collect(),
                },
            )
        })
        .collect();

    (headers, bodies)
}

mod file_client;
mod test_client;

pub use file_client::{FileClient, FileClientError};
pub use test_client::TestBodiesClient;
