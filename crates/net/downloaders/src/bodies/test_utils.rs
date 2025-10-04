//! Test helper impls for generating bodies

#![allow(dead_code)]

use alloy_consensus::BlockHeader;
use alloy_primitives::{B256, U256};
use reth_ethereum_primitives::BlockBody;
use reth_network_p2p::bodies::response::BlockResponse;
use reth_primitives_traits::{Block, SealedBlock, SealedHeader};
use reth_provider::{
    test_utils::MockNodeTypesWithDB, ProviderFactory, StaticFileProviderFactory, StaticFileSegment,
    StaticFileWriter,
};
use std::collections::HashMap;

pub(crate) fn zip_blocks<'a, B: Block>(
    headers: impl Iterator<Item = &'a SealedHeader<B::Header>>,
    bodies: &mut HashMap<B256, B::Body>,
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
    bodies: &mut HashMap<B256, BlockBody>,
) -> Vec<reth_ethereum_primitives::Block> {
    headers
        .into_iter()
        .map(|header| {
            let body = bodies.remove(&header.hash()).expect("body exists");
            body.into_block(header.unseal())
        })
        .collect()
}

#[inline]
pub(crate) fn insert_headers(
    factory: &ProviderFactory<MockNodeTypesWithDB>,
    headers: &[SealedHeader],
) {
    let provider_rw = factory.provider_rw().expect("failed to create provider");
    let static_file_provider = provider_rw.static_file_provider();
    let mut writer = static_file_provider
        .latest_writer(StaticFileSegment::Headers)
        .expect("failed to create writer");

    for header in headers {
        writer
            .append_header(header.header(), U256::ZERO, &header.hash())
            .expect("failed to append header");
    }
    drop(writer);
    provider_rw.commit().expect("failed to commit");
}
