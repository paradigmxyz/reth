//! Test helper impls

use async_trait::async_trait;
use reth_eth_wire::BlockBody;
use reth_interfaces::{
    p2p::{
        bodies::client::BodiesClient, download::DownloadClient, error::PeerRequestResult,
        priority::Priority,
    },
    test_utils::generators::random_block_range,
};
use reth_primitives::{PeerId, SealedHeader, H256};
use std::{
    collections::HashMap,
    fmt::Debug,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use tokio::sync::Mutex;

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

/// A [BodiesClient] for testing.
#[derive(Debug, Default)]
pub(crate) struct TestBodiesClient {
    bodies: Mutex<HashMap<H256, BlockBody>>,
    should_delay: bool,
    max_batch_size: Option<usize>,
    times_requested: AtomicU64,
}

impl TestBodiesClient {
    pub(crate) fn with_bodies(mut self, bodies: HashMap<H256, BlockBody>) -> Self {
        self.bodies = Mutex::new(bodies);
        self
    }

    pub(crate) fn with_should_delay(mut self, should_delay: bool) -> Self {
        self.should_delay = should_delay;
        self
    }

    pub(crate) fn with_max_batch_size(mut self, max_batch_size: usize) -> Self {
        self.max_batch_size = Some(max_batch_size);
        self
    }

    pub(crate) fn times_requested(&self) -> u64 {
        self.times_requested.load(Ordering::Relaxed)
    }
}

impl DownloadClient for TestBodiesClient {
    fn report_bad_message(&self, _peer_id: PeerId) {
        // noop
    }

    fn num_connected_peers(&self) -> usize {
        0
    }
}

#[async_trait]
impl BodiesClient for TestBodiesClient {
    async fn get_block_bodies_with_priority(
        &self,
        hashes: Vec<H256>,
        _priority: Priority,
    ) -> PeerRequestResult<Vec<BlockBody>> {
        if self.should_delay {
            tokio::time::sleep(Duration::from_millis(hashes[0].to_low_u64_be() % 100)).await;
        }
        self.times_requested.fetch_add(1, Ordering::Relaxed);
        let bodies = &mut *self.bodies.lock().await;
        Ok((
            PeerId::default(),
            hashes
                .into_iter()
                .take(self.max_batch_size.unwrap_or(usize::MAX))
                .map(|hash| {
                    bodies
                        .remove(&hash)
                        .expect("Downloader asked for a block it should not ask for")
                })
                .collect(),
        )
            .into())
    }
}
