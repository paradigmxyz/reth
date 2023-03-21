//! This includes download client implementations for auto sealing miners.
//!
//! Auto sealing miners can be polled, and will return a list of transactions that are ready to be
//! mined.
//!
//! TODO: this polls the miners, so maybe the consensus should also be implemented here?
//!
//! These downloaders poll the miner, assemble the block, and return transactions that are ready to
//! be mined.
use std::{
    collections::HashMap,
    fmt::Debug,
    pin::Pin,
    task::{Context, Poll},
};

use crate::mode::MiningMode;
use futures_util::Future;
use reth_interfaces::p2p::{
    bodies::client::{BodiesClient, BodiesFut},
    download::DownloadClient,
    headers::client::{HeadersClient, HeadersFut, HeadersRequest},
    priority::Priority,
};
// need to move this to primitives!!!
use reth_eth_wire::BlockBody;
use reth_primitives::{BlockHash, BlockNumber, Header, PeerId, H256};
use reth_transaction_pool::TransactionPool;
use tracing::warn;

/// A download client that polls the miner for transactions and assembles blocks to be returned in
/// the download process.
///
/// When polled, the miner will assemble blocks when miners produce ready transactions and store the
/// blocks in memory.
#[derive(Debug)]
pub struct AutoSealClient<Pool> {
    /// The active miner
    miner: MiningMode,

    /// Headers buffered for download.
    headers: HashMap<BlockNumber, Header>,

    /// A mapping between block hash and number.
    hash_to_number: HashMap<BlockHash, BlockNumber>,

    /// Bodies buffered for download.
    bodies: HashMap<BlockHash, BlockBody>,

    /// The mempool to poll for new ready transactions.
    pool: Pool,
}

impl<Pool> Future for AutoSealClient<Pool> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        while let Poll::Ready(txs) = Pin::new(&mut self.miner).poll(&self.pool, cx) {
            todo!("assemble block and put the header / body in maps")
        }
        Poll::Pending
    }
}

impl<Pool> HeadersClient for AutoSealClient<Pool>
where
    Pool: TransactionPool + Clone + 'static + Debug,
{
    type Output = HeadersFut;

    fn get_headers_with_priority(
        &self,
        request: HeadersRequest,
        _priority: Priority,
    ) -> Self::Output {
        todo!("return from map")
    }
}

impl<Pool> BodiesClient for AutoSealClient<Pool>
where
    Pool: TransactionPool + Clone + 'static + Debug,
{
    type Output = BodiesFut;

    fn get_block_bodies_with_priority(
        &self,
        hashes: Vec<H256>,
        priority: Priority,
    ) -> Self::Output {
        todo!("return from map")
    }
}

impl<Pool> DownloadClient for AutoSealClient<Pool>
where
    Pool: TransactionPool + Clone + 'static + Debug,
{
    fn report_bad_message(&self, _peer_id: PeerId) {
        warn!("Reported a bad message on a miner, we should never produce bad blocks");
        // noop
    }

    fn num_connected_peers(&self) -> usize {
        // no such thing as connected peers when we are mining ourselves
        1
    }
}
