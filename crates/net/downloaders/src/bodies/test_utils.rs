//! Test helper impls for generating bodies

#![allow(dead_code)]

use alloy_consensus::BlockHeader;
use alloy_primitives::map::B256Map;
use reth_ethereum_primitives::BlockBody;
use reth_network_p2p::bodies::response::BlockResponse;
use reth_primitives_traits::{Block, SealedBlock, SealedHeader};

pub(crate) fn zip_blocks<'a, B: Block>(
    headers: impl Iterator<Item = &'a SealedHeader<B::Header>>,
    bodies: &mut B256Map<B::Body>,
) -> Vec<BlockResponse<B>> {
    headers
        .into_iter()
        .map(|header| {
            let body = bodies.remove(&header.hash()).expect("body exists");
            if header.is_empty() {
                BlockResponse::Empty(header.clone())
            } else {
                BlockResponse::Full(SealedBlock::from_sealed_parts(header.clone(), body))
            }
        })
        .collect()
}

pub(crate) fn create_raw_bodies(
    headers: impl IntoIterator<Item = SealedHeader>,
    bodies: &mut B256Map<BlockBody>,
) -> Vec<reth_ethereum_primitives::Block> {
    headers
        .into_iter()
        .map(|header| {
            let body = bodies.remove(&header.hash()).expect("body exists");
            body.into_block(header.unseal())
        })
        .collect()
}
