//! The mode the consensus is operating in

use futures_util::{stream::Fuse, SinkExt, StreamExt};
use reth_interfaces::p2p::{headers::client::{HeadersClient, HeadersFut, HeadersRequest}, priority::Priority, download::DownloadClient, bodies::client::{BodiesClient, BodiesFut}};
use reth_primitives::{TxHash, PeerId, H256};
use std::{
    fmt,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};
use tokio::{sync::mpsc::Receiver, time::Interval};
use tokio_stream::{wrappers::ReceiverStream, Stream};
use tracing::{trace, warn};

/// Mode of operations for the `Miner`
#[derive(Debug)]
pub enum MiningMode {
    /// A miner that does nothing
    None,
    /// A miner that listens for new transactions that are ready.
    ///
    /// Either one transaction will be mined per block, or any number of transactions will be
    /// allowed
    Auto(ReadyTransactionMiner),
    /// A miner that constructs a new block every `interval` tick
    FixedBlockTime(FixedBlockTimeMiner),
}

// === impl MiningMode ===

impl MiningMode {
    pub fn instant(max_transactions: usize, listener: Receiver<TxHash>) -> Self {
        MiningMode::Auto(ReadyTransactionMiner {
            max_transactions,
            has_pending_txs: None,
            rx: ReceiverStream::new(listener).fuse(),
        })
    }

    pub fn interval(duration: Duration) -> Self {
        MiningMode::FixedBlockTime(FixedBlockTimeMiner::new(duration))
    }

    /// polls the [Pool] and returns those transactions that should be put in a block, if any.
    pub fn poll<Pool>(
        &mut self,
        pool: &Pool,
        cx: &mut Context<'_>,
    ) -> Poll<Vec<Arc<PoolTransaction>>> {
        match self {
            MiningMode::None => Poll::Pending,
            MiningMode::Auto(miner) => miner.poll(pool, cx),
            MiningMode::FixedBlockTime(miner) => miner.poll(pool, cx),
        }
    }
}

/// A miner that's supposed to create a new block every `interval`, mining all transactions that are
/// ready at that time.
///
/// The default blocktime is set to 6 seconds
#[derive(Debug)]
pub struct FixedBlockTimeMiner {
    /// The interval this fixed block time miner operates with
    interval: Interval,
}

// === impl FixedBlockTimeMiner ===

impl FixedBlockTimeMiner {
    /// Creates a new instance with an interval of `duration`
    pub fn new(duration: Duration) -> Self {
        let start = tokio::time::Instant::now() + duration;
        Self { interval: tokio::time::interval_at(start, duration) }
    }

    fn poll<P>(&mut self, pool: &P, cx: &mut Context<'_>) -> Poll<Vec<Arc<PoolTransaction>>>
    where
        P: 'static,
    {
        if self.interval.poll_tick(cx).is_ready() {
            // drain the pool
            return Poll::Ready(pool.ready_transactions().collect())
        }
        Poll::Pending
    }
}

impl Default for FixedBlockTimeMiner {
    fn default() -> Self {
        Self::new(Duration::from_secs(6))
    }
}

/// A miner that Listens for new ready transactions
pub struct ReadyTransactionMiner {
    /// how many transactions to mine per block
    max_transactions: usize,
    /// stores whether there are pending transactions (if known)
    has_pending_txs: Option<bool>,
    /// Receives hashes of transactions that are ready
    rx: Fuse<ReceiverStream<TxHash>>,
}

// === impl ReadyTransactionMiner ===

impl ReadyTransactionMiner {
    fn poll<Pool>(&mut self, pool: &Pool, cx: &mut Context<'_>) -> Poll<Vec<Arc<PoolTransaction>>> {
        // drain the notification stream
        while let Poll::Ready(Some(_hash)) = Pin::new(&mut self.rx).poll_next(cx) {
            self.has_pending_txs = Some(true);
        }

        if self.has_pending_txs == Some(false) {
            return Poll::Pending
        }

        let transactions =
            pool.ready_transactions().take(self.max_transactions).collect::<Vec<_>>();

        // there are pending transactions if we didn't drain the pool
        self.has_pending_txs = Some(transactions.len() >= self.max_transactions);

        if transactions.is_empty() {
            return Poll::Pending
        }

        Poll::Ready(transactions)
    }
}

impl fmt::Debug for ReadyTransactionMiner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReadyTransactionMiner")
            .field("max_transactions", &self.max_transactions)
            .finish_non_exhaustive()
    }
}

impl HeadersClient for ReadyTransactionMiner {
    type Output = HeadersFut;

    fn get_headers_with_priority(
        &self,
        request: HeadersRequest,
        _priority: Priority,
    ) -> Self::Output {
        todo!()
    }
}

impl BodiesClient for ReadyTransactionMiner {
    type Output = BodiesFut;

    fn get_block_bodies_with_priority(&self, hashes: Vec<H256>, priority: Priority) -> Self::Output {
        todo!()
    }
}

impl DownloadClient for ReadyTransactionMiner {
    fn report_bad_message(&self, _peer_id: PeerId) {
        warn!("Reported a bad message on a miner, we should never produce bad blocks");
        // noop
    }

    fn num_connected_peers(&self) -> usize {
        // no such thing as connected peers when we are mining ourselves
        1
    }
}
