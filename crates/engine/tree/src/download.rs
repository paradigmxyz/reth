//! Handler that can download blocks on demand (e.g. from the network).

use crate::{engine::DownloadRequest, metrics::BlockDownloaderMetrics};
use alloy_consensus::BlockHeader;
use alloy_primitives::{map::B256Set, B256};
use futures::FutureExt;
use reth_consensus::Consensus;
use reth_network_p2p::{
    full_block::{
        FetchFullBlockFuture, FetchFullBlockRangeFuture, FetchFullBlockRangeWithBalFuture,
        FetchFullBlockWithBalFuture, FullBlockClient, SealedBlockWithAccessList,
    },
    BlockAccessListsClient, BlockClient,
};
use reth_primitives_traits::{Block, SealedBlockWith};
use std::{
    cmp::{Ordering, Reverse},
    collections::{binary_heap::PeekMut, BinaryHeap, VecDeque},
    fmt::Debug,
    sync::Arc,
    task::{Context, Poll},
};
use tracing::trace;

/// A trait that can download blocks on demand.
pub trait BlockDownloader: Send + Sync {
    /// Type of the block being downloaded.
    type Block: Block;

    /// Handle an action.
    fn on_action(&mut self, action: DownloadAction);

    /// Advance in progress requests if any
    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<DownloadOutcome<Self::Block>>;
}

/// Actions that can be performed by the block downloader.
#[derive(Debug)]
pub enum DownloadAction {
    /// Stop downloading blocks.
    Clear,
    /// Download given blocks
    Download(DownloadRequest),
}

/// Outcome of downloaded blocks.
#[derive(Debug)]
pub enum DownloadOutcome<B: Block> {
    /// Downloaded blocks with optional block access list data.
    Blocks(Vec<SealedBlockWithAccessList<B>>),
    /// New download started.
    NewDownloadStarted {
        /// How many blocks are pending in this download.
        remaining_blocks: u64,
        /// The hash of the highest block of this download.
        target: B256,
    },
}

/// Basic [`BlockDownloader`].
#[expect(missing_debug_implementations)]
pub struct BasicBlockDownloader<Client, B: Block>
where
    Client: BlockClient + BlockAccessListsClient + 'static,
{
    /// A downloader that can download full blocks from the network.
    full_block_client: FullBlockClient<Client>,
    /// In-flight full block requests in progress.
    inflight_full_block_requests: Vec<FullBlockDownload<Client>>,
    /// In-flight full block _range_ requests in progress.
    inflight_block_range_requests: Vec<FullBlockRangeDownload<Client>>,
    /// Buffered blocks from downloads - this is a min-heap of blocks, using the block number for
    /// ordering. This means the blocks will be popped from the heap with ascending block numbers.
    set_buffered_blocks: BinaryHeap<Reverse<OrderedDownloadedBlock<B>>>,
    /// Engine download metrics.
    metrics: BlockDownloaderMetrics,
    /// Pending events to be emitted.
    pending_events: VecDeque<DownloadOutcome<B>>,
}

impl<Client, B> BasicBlockDownloader<Client, B>
where
    Client: BlockClient<Block = B> + BlockAccessListsClient + 'static,
    B: Block,
{
    /// Create a new instance
    pub fn new(client: Client, consensus: Arc<dyn Consensus<B>>) -> Self {
        Self {
            full_block_client: FullBlockClient::new(client, consensus),
            inflight_full_block_requests: Vec::new(),
            inflight_block_range_requests: Vec::new(),
            set_buffered_blocks: BinaryHeap::new(),
            metrics: BlockDownloaderMetrics::default(),
            pending_events: Default::default(),
        }
    }

    /// Clears the stored inflight requests.
    fn clear(&mut self) {
        self.inflight_full_block_requests.clear();
        self.inflight_block_range_requests.clear();
        self.set_buffered_blocks.clear();
        self.update_block_download_metrics();
    }

    /// Processes a download request.
    fn download(&mut self, request: DownloadRequest) {
        match request {
            DownloadRequest::BlockSet { hashes, access_lists } => {
                self.download_block_set(hashes, access_lists)
            }
            DownloadRequest::BlockRange { hash, count, access_lists } => {
                self.download_block_range(hash, count, access_lists)
            }
        }
    }

    /// Processes a block set download request.
    fn download_block_set(&mut self, hashes: B256Set, access_lists: bool) {
        for hash in hashes {
            self.download_full_block(hash, access_lists);
        }
    }

    /// Processes a block range download request.
    fn download_block_range(&mut self, hash: B256, count: u64, access_lists: bool) {
        if count == 1 {
            self.download_full_block(hash, access_lists);
        } else {
            trace!(
                target: "engine::download",
                ?hash,
                ?count,
                access_lists,
                "start downloading full block range."
            );

            let request = if access_lists {
                FullBlockRangeDownload::WithAccessLists(
                    self.full_block_client
                        .get_full_block_range_with_optional_access_lists(hash, count),
                )
            } else {
                FullBlockRangeDownload::Blocks(
                    self.full_block_client.get_full_block_range(hash, count),
                )
            };
            self.push_pending_event(DownloadOutcome::NewDownloadStarted {
                remaining_blocks: request.count(),
                target: request.start_hash(),
            });
            self.inflight_block_range_requests.push(request);

            self.update_block_download_metrics();
        }
    }

    /// Starts requesting a full block from the network.
    ///
    /// Returns `true` if the request was started, `false` if there's already a request for the
    /// given hash.
    fn download_full_block(&mut self, hash: B256, access_lists: bool) -> bool {
        if self.is_inflight_request(hash) {
            return false
        }
        self.push_pending_event(DownloadOutcome::NewDownloadStarted {
            remaining_blocks: 1,
            target: hash,
        });

        trace!(
            target: "engine::download",
            ?hash,
            access_lists,
            "Start downloading full block"
        );

        let request = if access_lists {
            FullBlockDownload::WithAccessList(
                self.full_block_client.get_full_block_with_access_lists(hash),
            )
        } else {
            FullBlockDownload::Block(self.full_block_client.get_full_block(hash))
        };
        self.inflight_full_block_requests.push(request);

        self.update_block_download_metrics();

        true
    }

    /// Returns true if there's already a request for the given hash.
    fn is_inflight_request(&self, hash: B256) -> bool {
        self.inflight_full_block_requests.iter().any(|req| *req.hash() == hash)
    }

    /// Sets the metrics for the active downloads
    fn update_block_download_metrics(&self) {
        let blocks = self.inflight_full_block_requests.len() +
            self.inflight_block_range_requests.iter().map(|r| r.count() as usize).sum::<usize>();
        self.metrics.active_block_downloads.set(blocks as f64);
    }

    /// Adds a pending event to the FIFO queue.
    fn push_pending_event(&mut self, pending_event: DownloadOutcome<B>) {
        self.pending_events.push_back(pending_event);
    }

    /// Removes a pending event from the FIFO queue.
    fn pop_pending_event(&mut self) -> Option<DownloadOutcome<B>> {
        self.pending_events.pop_front()
    }
}

impl<Client, B> BlockDownloader for BasicBlockDownloader<Client, B>
where
    Client: BlockClient<Block = B> + BlockAccessListsClient,
    B: Block,
{
    type Block = B;

    /// Handles incoming download actions.
    fn on_action(&mut self, action: DownloadAction) {
        match action {
            DownloadAction::Clear => self.clear(),
            DownloadAction::Download(request) => self.download(request),
        }
    }

    /// Advances the download process.
    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<DownloadOutcome<B>> {
        if let Some(pending_event) = self.pop_pending_event() {
            return Poll::Ready(pending_event);
        }

        // advance all full block requests
        for idx in (0..self.inflight_full_block_requests.len()).rev() {
            let mut request = self.inflight_full_block_requests.swap_remove(idx);
            if let Poll::Ready(block) = request.poll(cx) {
                trace!(target: "engine::download", block=?block.block().num_hash(), "Received single full block, buffering");
                self.set_buffered_blocks.push(Reverse(block.into()));
            } else {
                // still pending
                self.inflight_full_block_requests.push(request);
            }
        }

        // advance all full block range requests
        for idx in (0..self.inflight_block_range_requests.len()).rev() {
            let mut request = self.inflight_block_range_requests.swap_remove(idx);
            if let Poll::Ready(blocks) = request.poll(cx) {
                trace!(target: "engine::download", len=?blocks.len(), first=?blocks.first().map(|b| b.block().num_hash()), last=?blocks.last().map(|b| b.block().num_hash()), "Received full block range, buffering");
                self.set_buffered_blocks
                    .extend(blocks.into_iter().map(OrderedDownloadedBlock).map(Reverse));
            } else {
                // still pending
                self.inflight_block_range_requests.push(request);
            }
        }

        self.update_block_download_metrics();

        if self.set_buffered_blocks.is_empty() {
            return Poll::Pending;
        }

        // drain all unique element of the block buffer if there are any
        let mut downloaded_blocks = Vec::with_capacity(self.set_buffered_blocks.len());
        while let Some(block) = self.set_buffered_blocks.pop() {
            let mut block = block.0 .0;
            // peek ahead and pop duplicates, keeping the copy that includes access list data
            while let Some(peek) = self.set_buffered_blocks.peek_mut() {
                if peek.0 .0.block().hash() == block.block().hash() {
                    let duplicate = PeekMut::pop(peek).0 .0;
                    if block.data().is_none() && duplicate.data().is_some() {
                        block = duplicate;
                    }
                } else {
                    break
                }
            }
            downloaded_blocks.push(block);
        }
        Poll::Ready(DownloadOutcome::Blocks(downloaded_blocks))
    }
}

/// A wrapper type around [`SealedBlockWithAccessList`] that implements the [Ord]
/// trait by block number.
#[derive(Debug, Clone, PartialEq, Eq)]
struct OrderedDownloadedBlock<B: Block>(SealedBlockWithAccessList<B>);

impl<B: Block> PartialOrd for OrderedDownloadedBlock<B> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<B: Block> Ord for OrderedDownloadedBlock<B> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.block().number().cmp(&other.0.block().number())
    }
}

impl<B: Block> From<SealedBlockWithAccessList<B>> for OrderedDownloadedBlock<B> {
    fn from(block: SealedBlockWithAccessList<B>) -> Self {
        Self(block)
    }
}

impl<B: Block> From<OrderedDownloadedBlock<B>> for SealedBlockWithAccessList<B> {
    fn from(value: OrderedDownloadedBlock<B>) -> Self {
        value.0
    }
}

/// An in-flight full block request that optionally also fetches the block's access list.
enum FullBlockDownload<Client>
where
    Client: BlockClient + BlockAccessListsClient,
{
    /// Fetches the block only.
    Block(FetchFullBlockFuture<Client>),
    /// Fetches the block and attempts to fetch its access list.
    WithAccessList(FetchFullBlockWithBalFuture<Client>),
}

impl<Client> FullBlockDownload<Client>
where
    Client: BlockClient + BlockAccessListsClient + 'static,
{
    /// Returns the hash of the block being requested.
    const fn hash(&self) -> &B256 {
        match self {
            Self::Block(req) => req.hash(),
            Self::WithAccessList(req) => req.hash(),
        }
    }

    /// Advances the download.
    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<SealedBlockWithAccessList<Client::Block>> {
        match self {
            Self::Block(req) => req.poll_unpin(cx).map(SealedBlockWith::from_block),
            Self::WithAccessList(req) => req.poll_unpin(cx),
        }
    }
}

/// An in-flight full block range request that optionally also fetches the blocks' access lists.
enum FullBlockRangeDownload<Client>
where
    Client: BlockClient + BlockAccessListsClient,
{
    /// Fetches the block range only.
    Blocks(FetchFullBlockRangeFuture<Client>),
    /// Fetches the block range and attempts to fetch the blocks' access lists.
    WithAccessLists(FetchFullBlockRangeWithBalFuture<Client>),
}

impl<Client> FullBlockRangeDownload<Client>
where
    Client: BlockClient + BlockAccessListsClient + 'static,
{
    /// Returns the block hash the requested range starts at (inclusive).
    const fn start_hash(&self) -> B256 {
        match self {
            Self::Blocks(req) => req.start_hash(),
            Self::WithAccessLists(req) => req.start_hash(),
        }
    }

    /// Returns the number of requested blocks.
    const fn count(&self) -> u64 {
        match self {
            Self::Blocks(req) => req.count(),
            Self::WithAccessLists(req) => req.count(),
        }
    }

    /// Advances the download.
    fn poll(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Vec<SealedBlockWithAccessList<Client::Block>>> {
        match self {
            Self::Blocks(req) => req
                .poll_unpin(cx)
                .map(|blocks| blocks.into_iter().map(SealedBlockWith::from_block).collect()),
            Self::WithAccessLists(req) => req.poll_unpin(cx),
        }
    }
}

/// A [`BlockDownloader`] that does nothing.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct NoopBlockDownloader<B>(core::marker::PhantomData<B>);

impl<B: Block> BlockDownloader for NoopBlockDownloader<B> {
    type Block = B;

    fn on_action(&mut self, _event: DownloadAction) {}

    fn poll(&mut self, _cx: &mut Context<'_>) -> Poll<DownloadOutcome<B>> {
        Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::insert_headers_into_client;
    use alloy_consensus::Header;
    use alloy_eips::eip1559::ETHEREUM_BLOCK_GAS_LIMIT_30M;
    use assert_matches::assert_matches;
    use reth_chainspec::{ChainSpecBuilder, MAINNET};
    use reth_ethereum_consensus::EthBeaconConsensus;
    use reth_network_p2p::test_utils::TestFullBlockClient;
    use reth_primitives_traits::SealedHeader;
    use std::{future::poll_fn, sync::Arc};

    struct TestHarness {
        block_downloader:
            BasicBlockDownloader<TestFullBlockClient, reth_ethereum_primitives::Block>,
        client: TestFullBlockClient,
    }

    impl TestHarness {
        fn new(total_blocks: usize) -> Self {
            let chain_spec = Arc::new(
                ChainSpecBuilder::default()
                    .chain(MAINNET.chain)
                    .genesis(MAINNET.genesis.clone())
                    .paris_activated()
                    .build(),
            );

            let client = TestFullBlockClient::default();
            let header = Header {
                base_fee_per_gas: Some(7),
                gas_limit: ETHEREUM_BLOCK_GAS_LIMIT_30M,
                ..Default::default()
            };
            let header = SealedHeader::seal_slow(header);

            insert_headers_into_client(&client, header, 0..total_blocks);
            let consensus = Arc::new(EthBeaconConsensus::new(chain_spec));

            let block_downloader = BasicBlockDownloader::new(client.clone(), consensus);
            Self { block_downloader, client }
        }
    }

    #[tokio::test]
    async fn block_downloader_range_request() {
        const TOTAL_BLOCKS: usize = 10;
        let TestHarness { mut block_downloader, client } = TestHarness::new(TOTAL_BLOCKS);
        let tip = client.highest_block().expect("there should be blocks here");

        // send block range download request
        block_downloader.on_action(DownloadAction::Download(DownloadRequest::block_range(
            tip.hash(),
            tip.number,
        )));

        // ensure we have one in flight range request
        assert_eq!(block_downloader.inflight_block_range_requests.len(), 1);

        // ensure the range request is made correctly
        let first_req = block_downloader.inflight_block_range_requests.first().unwrap();
        assert_eq!(first_req.start_hash(), tip.hash());
        assert_eq!(first_req.count(), tip.number);

        // poll downloader
        let sync_future = poll_fn(|cx| block_downloader.poll(cx));
        let next_ready = sync_future.await;

        assert_matches!(next_ready, DownloadOutcome::NewDownloadStarted { remaining_blocks, .. } => {
            assert_eq!(remaining_blocks, TOTAL_BLOCKS as u64);
        });

        let sync_future = poll_fn(|cx| block_downloader.poll(cx));
        let next_ready = sync_future.await;

        assert_matches!(next_ready, DownloadOutcome::Blocks(blocks) => {
            // ensure all blocks were obtained
            assert_eq!(blocks.len(), TOTAL_BLOCKS);

            // ensure they are in ascending order
            for num in 1..=TOTAL_BLOCKS {
                assert_eq!(blocks[num - 1].block().number(), num as u64);
            }
        });
    }

    #[tokio::test]
    async fn block_downloader_set_request() {
        const TOTAL_BLOCKS: usize = 2;
        let TestHarness { mut block_downloader, client } = TestHarness::new(TOTAL_BLOCKS);

        let tip = client.highest_block().expect("there should be blocks here");

        // send block set download request
        block_downloader.on_action(DownloadAction::Download(DownloadRequest::block_set(
            B256Set::from_iter([tip.hash(), tip.parent_hash]),
        )));

        // ensure we have TOTAL_BLOCKS in flight full block request
        assert_eq!(block_downloader.inflight_full_block_requests.len(), TOTAL_BLOCKS);

        // poll downloader
        for _ in 0..TOTAL_BLOCKS {
            let sync_future = poll_fn(|cx| block_downloader.poll(cx));
            let next_ready = sync_future.await;

            assert_matches!(next_ready, DownloadOutcome::NewDownloadStarted { remaining_blocks, .. } => {
                assert_eq!(remaining_blocks, 1);
            });
        }

        let sync_future = poll_fn(|cx| block_downloader.poll(cx));
        let next_ready = sync_future.await;
        assert_matches!(next_ready, DownloadOutcome::Blocks(blocks) => {
            // ensure all blocks were obtained
            assert_eq!(blocks.len(), TOTAL_BLOCKS);

            // ensure they are in ascending order
            for num in 1..=TOTAL_BLOCKS {
                assert_eq!(blocks[num - 1].block().number(), num as u64);
            }
        });
    }

    #[tokio::test]
    async fn block_downloader_range_request_with_access_lists() {
        const TOTAL_BLOCKS: usize = 4;
        let chain_spec = Arc::new(
            ChainSpecBuilder::default()
                .chain(MAINNET.chain)
                .genesis(MAINNET.genesis.clone())
                .paris_activated()
                .build(),
        );

        let client = TestFullBlockClient::default();
        // empty RLP list
        let access_list = alloy_primitives::Bytes::from_static(&[0xc0]);
        let header = Header {
            base_fee_per_gas: Some(7),
            gas_limit: ETHEREUM_BLOCK_GAS_LIMIT_30M,
            block_access_list_hash: Some(alloy_primitives::keccak256(access_list.as_ref())),
            ..Default::default()
        };
        let mut sealed_header = SealedHeader::seal_slow(header);
        let body = reth_ethereum_primitives::BlockBody::default();
        for _ in 0..TOTAL_BLOCKS {
            let (mut header, hash) = sealed_header.split();
            header.parent_hash = hash;
            header.number += 1;
            header.timestamp += 1;
            sealed_header = SealedHeader::seal_slow(header);
            client.insert(sealed_header.clone(), body.clone());
            client.insert_access_list(sealed_header.hash(), access_list.clone());
        }

        let consensus = Arc::new(EthBeaconConsensus::new(chain_spec).with_allow_bal_hashes(true));
        let mut block_downloader = BasicBlockDownloader::new(client.clone(), consensus);

        let tip = client.highest_block().expect("there should be blocks here");

        block_downloader.on_action(DownloadAction::Download(
            DownloadRequest::block_range(tip.hash(), tip.number).with_access_lists(true),
        ));

        let sync_future = poll_fn(|cx| block_downloader.poll(cx));
        let next_ready = sync_future.await;

        assert_matches!(next_ready, DownloadOutcome::NewDownloadStarted { remaining_blocks, .. } => {
            assert_eq!(remaining_blocks, TOTAL_BLOCKS as u64);
        });

        let sync_future = poll_fn(|cx| block_downloader.poll(cx));
        let next_ready = sync_future.await;

        assert_matches!(next_ready, DownloadOutcome::Blocks(blocks) => {
            assert_eq!(blocks.len(), TOTAL_BLOCKS);

            // every block carries its validated access list data
            for block in &blocks {
                assert!(block.data().is_some());
            }
        });
    }

    #[tokio::test]
    async fn block_downloader_clear_request() {
        const TOTAL_BLOCKS: usize = 10;
        let TestHarness { mut block_downloader, client } = TestHarness::new(TOTAL_BLOCKS);

        let tip = client.highest_block().expect("there should be blocks here");

        // send block range download request
        block_downloader.on_action(DownloadAction::Download(DownloadRequest::block_range(
            tip.hash(),
            tip.number,
        )));

        // send block set download request
        let download_set = B256Set::from_iter([tip.hash(), tip.parent_hash]);
        block_downloader
            .on_action(DownloadAction::Download(DownloadRequest::block_set(download_set.clone())));

        // ensure we have one in flight range request
        assert_eq!(block_downloader.inflight_block_range_requests.len(), 1);

        // ensure the range request is made correctly
        let first_req = block_downloader.inflight_block_range_requests.first().unwrap();
        assert_eq!(first_req.start_hash(), tip.hash());
        assert_eq!(first_req.count(), tip.number);

        // ensure we have download_set.len() in flight full block request
        assert_eq!(block_downloader.inflight_full_block_requests.len(), download_set.len());

        // send clear request
        block_downloader.on_action(DownloadAction::Clear);

        // ensure we have no in flight range request
        assert_eq!(block_downloader.inflight_block_range_requests.len(), 0);

        // ensure we have no in flight full block request
        assert_eq!(block_downloader.inflight_full_block_requests.len(), 0);
    }
}
