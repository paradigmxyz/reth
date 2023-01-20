//! Test helper impls

use async_trait::async_trait;
use reth_eth_wire::BlockBody;
use reth_interfaces::{
    p2p::{
        bodies::client::BodiesClient, downloader::DownloadClient, error::PeerRequestResult,
        priority::Priority,
    },
    test_utils::generators::random_block_range,
};
use reth_primitives::{PeerId, SealedHeader, H256};
use std::{
    collections::HashMap,
    fmt::{Debug, Formatter},
    future::Future,
    sync::Arc,
};
use tokio::sync::Mutex;

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
pub(crate) struct TestBodiesClient<F>(pub(crate) Arc<Mutex<F>>);

impl<F> Debug for TestBodiesClient<F> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestBodiesClient").finish_non_exhaustive()
    }
}

impl<F> TestBodiesClient<F> {
    pub(crate) fn new(f: F) -> Self {
        Self(Arc::new(Mutex::new(f)))
    }
}

impl<F: Send + Sync> DownloadClient for TestBodiesClient<F> {
    fn report_bad_message(&self, _peer_id: PeerId) {
        // noop
    }

    fn num_connected_peers(&self) -> usize {
        0
    }
}

#[async_trait]
impl<F, Fut> BodiesClient for TestBodiesClient<F>
where
    F: FnMut(Vec<H256>) -> Fut + Send + Sync,
    Fut: Future<Output = PeerRequestResult<Vec<BlockBody>>> + Send,
{
    async fn get_block_bodies_with_priority(
        &self,
        hash: Vec<H256>,
        _priority: Priority,
    ) -> PeerRequestResult<Vec<BlockBody>> {
        let f = &mut *self.0.lock().await;
        (f)(hash).await
    }
}
