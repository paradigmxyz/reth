use crate::p2p::{
    bodies::client::{BodiesClient, BodiesFut},
    download::DownloadClient,
    error::PeerRequestResult,
    priority::Priority,
};
use async_trait::async_trait;
use futures::{future, Future, FutureExt};
use reth_eth_wire::BlockBody;
use reth_primitives::{WithPeerId, H256};
use std::{
    fmt::{Debug, Formatter},
    pin::Pin,
};
use tokio::sync::oneshot::{self, Receiver};

/// A test client for fetching bodies
pub struct TestBodiesClient<F> {
    /// The function that is called on each body request.
    pub responder: F,
}

impl<F> Debug for TestBodiesClient<F> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestBodiesClient").finish_non_exhaustive()
    }
}

impl<F: Sync + Send> DownloadClient for TestBodiesClient<F> {
    fn report_bad_message(&self, _peer_id: reth_primitives::PeerId) {
        // noop
    }

    fn num_connected_peers(&self) -> usize {
        0
    }
}

impl<F> BodiesClient for TestBodiesClient<F>
where
    F: Fn(Vec<H256>) -> PeerRequestResult<Vec<BlockBody>> + Send + Sync,
{
    type Output = BodiesFut;

    fn get_block_bodies_with_priority(
        &self,
        hashes: Vec<H256>,
        _priority: Priority,
    ) -> Self::Output {
        let (tx, rx) = oneshot::channel();
        tx.send((self.responder)(hashes));
        Box::pin(rx.map(|x| match x {
            Ok(value) => value,
            Err(err) => Err(err.into()),
        }))
    }
}
