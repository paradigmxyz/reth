//! Test helper impls.

#![allow(dead_code)]

use crate::{bodies::test_utils::create_raw_bodies, file_codec::BlockFileCodec};
use alloy_primitives::B256;
use futures::SinkExt;
use reth_primitives::{BlockBody, SealedHeader};
use reth_testing_utils::generators::{self, random_block_range, BlockRangeParams};
use std::{collections::HashMap, io::SeekFrom, ops::RangeInclusive};
use tokio::{fs::File, io::AsyncSeekExt};
use tokio_util::codec::FramedWrite;

mod bodies_client;
pub use bodies_client::TestBodiesClient;

/// Metrics scope used for testing.
pub(crate) const TEST_SCOPE: &str = "downloaders.test";

/// Generate a set of bodies and their corresponding block hashes
pub(crate) fn generate_bodies(
    range: RangeInclusive<u64>,
) -> (Vec<SealedHeader>, HashMap<B256, BlockBody>) {
    let mut rng = generators::rng();
    let blocks = random_block_range(
        &mut rng,
        range,
        BlockRangeParams { parent: Some(B256::ZERO), tx_count: 0..2, ..Default::default() },
    );

    let headers = blocks.iter().map(|block| block.header.clone()).collect();
    let bodies = blocks.into_iter().map(|block| (block.hash(), block.body)).collect();

    (headers, bodies)
}

/// Generate a set of bodies, write them to a temporary file, and return the file along with the
/// bodies and corresponding block hashes
pub(crate) async fn generate_bodies_file(
    range: RangeInclusive<u64>,
) -> (tokio::fs::File, Vec<SealedHeader>, HashMap<B256, BlockBody>) {
    let (headers, bodies) = generate_bodies(range);
    let raw_block_bodies = create_raw_bodies(headers.iter().cloned(), &mut bodies.clone());

    let file: File = tempfile::tempfile().unwrap().into();
    let mut writer = FramedWrite::new(file, BlockFileCodec);

    // rlp encode one after the other
    for block in raw_block_bodies {
        writer.feed(block).await.unwrap();
    }
    writer.flush().await.unwrap();

    // get the file back
    let mut file: File = writer.into_inner();
    file.seek(SeekFrom::Start(0)).await.unwrap();
    (file, headers, bodies)
}
