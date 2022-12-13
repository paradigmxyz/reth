//! Test helper impls

use async_trait::async_trait;
use reth_eth_wire::BlockBody;
use reth_interfaces::{
    p2p::{bodies::client::BodiesClient, error::PeerRequestResult},
    test_utils::generators::random_block_range,
};
use reth_primitives::{BlockNumber, H256};
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
) -> (Vec<(BlockNumber, H256)>, HashMap<H256, BlockBody>) {
    let blocks = random_block_range(rng, H256::zero());

    let hashes: Vec<(BlockNumber, H256)> =
        blocks.iter().map(|block| (block.number, block.hash())).collect();
    let bodies: HashMap<H256, BlockBody> = blocks
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

    (hashes, bodies)
}

/// A [BodiesClient] for testing.
pub(crate) struct TestClient<F>(pub(crate) Arc<Mutex<F>>);

impl<F> Debug for TestClient<F> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestClient").finish_non_exhaustive()
    }
}

impl<F> TestClient<F> {
    pub(crate) fn new(f: F) -> Self {
        Self(Arc::new(Mutex::new(f)))
    }
}

#[async_trait]
impl<F, Fut> BodiesClient for TestClient<F>
where
    F: FnMut(Vec<H256>) -> Fut + Send + Sync,
    Fut: Future<Output = PeerRequestResult<Vec<BlockBody>>> + Send,
{
    async fn get_block_body(&self, hash: Vec<H256>) -> PeerRequestResult<Vec<BlockBody>> {
        let f = &mut *self.0.lock().await;
        (f)(hash).await
    }
}
