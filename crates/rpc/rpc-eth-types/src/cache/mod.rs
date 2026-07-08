//! Async caching support for eth RPC

use super::{EthStateCacheConfig, MultiConsumerLruCache};
use crate::block::CachedTransaction;
use alloy_consensus::{transaction::TxHashRef, BlockHeader};
use alloy_eip7928::bal::DecodedBal;
use alloy_eips::BlockHashOrNumber;
use alloy_primitives::{Address, TxHash, B256};
use futures::{stream::FuturesOrdered, Stream, StreamExt};
use reth_chain_state::CanonStateNotification;
use reth_errors::{ProviderError, ProviderResult};
use reth_execution_types::Chain;
use reth_primitives_traits::{Block, BlockBody, InMemorySize, NodePrimitives, RecoveredBlock};
use reth_revm::{
    bytecode::Bytecode,
    primitives::{StorageKey, StorageValue},
    state::bal::{
        AccountBal as RevmAccountBal, AccountInfoBal as RevmAccountInfoBal, Bal as RevmBal,
        BalWrites as RevmBalWrites, StorageBal as RevmStorageBal,
    },
};
use reth_storage_api::{BalProvider, BlockReader, TransactionVariant};
use reth_tasks::Runtime;
use schnellru::{ByLength, Limiter, LruMap};
use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::sync::{
    mpsc::{unbounded_channel, UnboundedSender},
    oneshot, Semaphore,
};
use tokio_stream::wrappers::UnboundedReceiverStream;

pub mod config;
pub mod db;
pub mod metrics;
pub mod multi_consumer;

/// The type that can send the response to a requested [`RecoveredBlock`]
type BlockWithSendersResponseSender<B> =
    oneshot::Sender<ProviderResult<Option<Arc<RecoveredBlock<B>>>>>;

/// The type that can send the response to the requested receipts of a block.
type ReceiptsResponseSender<R> = oneshot::Sender<ProviderResult<Option<Arc<Vec<R>>>>>;

type CachedBlockResponseSender<B> = oneshot::Sender<Option<Arc<RecoveredBlock<B>>>>;

type CachedBlockAndReceiptsResponseSender<B, R> =
    oneshot::Sender<(Option<Arc<RecoveredBlock<B>>>, Option<Arc<Vec<R>>>)>;

/// The type that can send the response to a requested header
type HeaderResponseSender<H> = oneshot::Sender<ProviderResult<H>>;

/// The type that can send the response with a chain of cached blocks
type CachedParentBlocksResponseSender<B> = oneshot::Sender<Vec<Arc<RecoveredBlock<B>>>>;

/// The type that can send the response for a transaction hash lookup
type TransactionHashResponseSender<B, R> = oneshot::Sender<Option<CachedTransaction<B, R>>>;

/// The type that can send the response to a requested revm BAL.
type BalResponseSender = oneshot::Sender<ProviderResult<Option<CachedRevmBal>>>;

type BlockLruCache<B, L> =
    MultiConsumerLruCache<B256, Arc<RecoveredBlock<B>>, L, BlockWithSendersResponseSender<B>>;

type ReceiptsLruCache<R, L> =
    MultiConsumerLruCache<B256, Arc<Vec<R>>, L, ReceiptsResponseSender<R>>;

type HeaderLruCache<H, L> = MultiConsumerLruCache<B256, H, L, HeaderResponseSender<H>>;

type BalLruCache<L> = MultiConsumerLruCache<B256, CachedRevmBal, L, BalResponseSender>;

/// Provides async access to cached eth data
///
/// This is the frontend for the async caching service which manages cached data on a different
/// task.
#[derive(Debug)]
pub struct EthStateCache<N: NodePrimitives> {
    to_service: UnboundedSender<CacheAction<N::Block, N::Receipt>>,
}

impl<N: NodePrimitives> Clone for EthStateCache<N> {
    fn clone(&self) -> Self {
        Self { to_service: self.to_service.clone() }
    }
}

impl<N: NodePrimitives> EthStateCache<N> {
    /// Creates and returns both [`EthStateCache`] frontend and the memory bound service.
    fn create<Provider>(
        provider: Provider,
        action_task_spawner: Runtime,
        config: EthStateCacheConfig,
    ) -> (Self, EthStateCacheService<Provider, Runtime>)
    where
        Provider: BlockReader<Block = N::Block, Receipt = N::Receipt> + BalProvider,
    {
        let EthStateCacheConfig {
            max_blocks,
            max_receipts,
            max_headers,
            max_bals,
            max_concurrent_db_requests,
            max_cached_tx_hashes,
        } = config;
        let (to_service, rx) = unbounded_channel();

        let service = EthStateCacheService {
            provider,
            full_block_cache: BlockLruCache::new(max_blocks, "blocks"),
            receipts_cache: ReceiptsLruCache::new(max_receipts, "receipts"),
            headers_cache: HeaderLruCache::new(max_headers, "headers"),
            bal_cache: BalLruCache::new(max_bals, "bals"),
            action_tx: to_service.clone(),
            action_rx: UnboundedReceiverStream::new(rx),
            action_task_spawner,
            rate_limiter: Arc::new(Semaphore::new(max_concurrent_db_requests)),
            tx_hash_index: LruMap::new(ByLength::new(max_cached_tx_hashes)),
        };
        let cache = Self { to_service };
        (cache, service)
    }

    /// Creates a new async LRU backed cache service task and spawns it to a new task via the given
    /// spawner.
    ///
    /// The cache is memory limited by the given max bytes values.
    pub fn spawn_with<Provider>(
        provider: Provider,
        config: EthStateCacheConfig,
        executor: Runtime,
    ) -> Self
    where
        Provider: BlockReader<Block = N::Block, Receipt = N::Receipt>
            + BalProvider
            + Clone
            + Unpin
            + 'static,
    {
        let (this, service) = Self::create(provider, executor.clone(), config);
        executor.spawn_critical_task("eth state cache", service);
        this
    }

    /// Requests the  [`RecoveredBlock`] for the block hash
    ///
    /// Returns `None` if the block does not exist.
    pub async fn get_recovered_block(
        &self,
        block_hash: B256,
    ) -> ProviderResult<Option<Arc<RecoveredBlock<N::Block>>>> {
        let (response_tx, rx) = oneshot::channel();
        let _ = self.to_service.send(CacheAction::GetBlockWithSenders { block_hash, response_tx });
        rx.await.map_err(|_| CacheServiceUnavailable)?
    }

    /// Requests the receipts for the block hash
    ///
    /// Returns `None` if the block was not found.
    pub async fn get_receipts(
        &self,
        block_hash: B256,
    ) -> ProviderResult<Option<Arc<Vec<N::Receipt>>>> {
        let (response_tx, rx) = oneshot::channel();
        let _ = self.to_service.send(CacheAction::GetReceipts { block_hash, response_tx });
        rx.await.map_err(|_| CacheServiceUnavailable)?
    }

    /// Fetches both receipts and block for the given block hash.
    pub async fn get_block_and_receipts(
        &self,
        block_hash: B256,
    ) -> ProviderResult<Option<(Arc<RecoveredBlock<N::Block>>, Arc<Vec<N::Receipt>>)>> {
        let block = self.get_recovered_block(block_hash);
        let receipts = self.get_receipts(block_hash);

        let (block, receipts) = futures::try_join!(block, receipts)?;

        Ok(block.zip(receipts))
    }

    /// Retrieves receipts and blocks from cache if block is in the cache, otherwise only receipts.
    pub async fn get_receipts_and_maybe_block(
        &self,
        block_hash: B256,
    ) -> ProviderResult<Option<(Arc<Vec<N::Receipt>>, Option<Arc<RecoveredBlock<N::Block>>>)>> {
        let (response_tx, rx) = oneshot::channel();
        let _ = self.to_service.send(CacheAction::GetCachedBlock { block_hash, response_tx });

        let receipts = self.get_receipts(block_hash);

        let (receipts, block) = futures::join!(receipts, rx);

        let block = block.map_err(|_| CacheServiceUnavailable)?;
        Ok(receipts?.map(|r| (r, block)))
    }

    /// Retrieves both block and receipts from cache if available.
    pub async fn maybe_cached_block_and_receipts(
        &self,
        block_hash: B256,
    ) -> ProviderResult<(Option<Arc<RecoveredBlock<N::Block>>>, Option<Arc<Vec<N::Receipt>>>)> {
        let (response_tx, rx) = oneshot::channel();
        let _ = self
            .to_service
            .send(CacheAction::GetCachedBlockAndReceipts { block_hash, response_tx });
        rx.await.map_err(|_| CacheServiceUnavailable.into())
    }

    /// Streams cached receipts and blocks for a list of block hashes, preserving input order.
    #[expect(clippy::type_complexity)]
    pub fn get_receipts_and_maybe_block_stream<'a>(
        &'a self,
        hashes: Vec<B256>,
    ) -> impl Stream<
        Item = ProviderResult<
            Option<(Arc<Vec<N::Receipt>>, Option<Arc<RecoveredBlock<N::Block>>>)>,
        >,
    > + 'a {
        let futures = hashes.into_iter().map(move |hash| self.get_receipts_and_maybe_block(hash));

        futures.collect::<FuturesOrdered<_>>()
    }

    /// Requests the header for the given hash.
    ///
    /// Returns an error if the header is not found.
    pub async fn get_header(&self, block_hash: B256) -> ProviderResult<N::BlockHeader> {
        let (response_tx, rx) = oneshot::channel();
        let _ = self.to_service.send(CacheAction::GetHeader { block_hash, response_tx });
        rx.await.map_err(|_| CacheServiceUnavailable)?
    }

    /// Retrieves a chain of connected blocks from the cache, starting from the given block hash
    /// and traversing down through parent hashes. Returns blocks in descending order (newest
    /// first).
    /// This is useful for efficiently retrieving a sequence of blocks that might already be in
    /// cache without making separate database requests.
    /// Returns `None` if no blocks are found in the cache, otherwise returns `Some(Vec<...>)`
    /// with at least one block.
    pub async fn get_cached_parent_blocks(
        &self,
        block_hash: B256,
        max_blocks: usize,
    ) -> Option<Vec<Arc<RecoveredBlock<N::Block>>>> {
        let (response_tx, rx) = oneshot::channel();
        let _ = self.to_service.send(CacheAction::GetCachedParentBlocks {
            block_hash,
            max_blocks,
            response_tx,
        });

        let blocks = rx.await.unwrap_or_default();
        if blocks.is_empty() {
            None
        } else {
            Some(blocks)
        }
    }

    /// Looks up a transaction by its hash in the cache index.
    ///
    /// Returns the cached block, transaction index, and optionally receipts if the transaction
    /// is in a cached block.
    pub async fn get_transaction_by_hash(
        &self,
        tx_hash: TxHash,
    ) -> Option<CachedTransaction<N::Block, N::Receipt>> {
        let (response_tx, rx) = oneshot::channel();
        let _ = self.to_service.send(CacheAction::GetTransactionByHash { tx_hash, response_tx });
        rx.await.ok()?
    }

    /// Requests the revm BAL for the block hash.
    ///
    /// Returns `None` if the BAL does not exist.
    pub async fn get_bal(
        &self,
        block_hash: B256,
    ) -> ProviderResult<Option<Arc<DecodedBal<Arc<RevmBal>>>>> {
        let (response_tx, rx) = oneshot::channel();
        let _ = self.to_service.send(CacheAction::GetBal { block_hash, response_tx });
        rx.await
            .map_err(|_| CacheServiceUnavailable)?
            .map(|maybe_bal| maybe_bal.map(|cached| cached.0))
    }
}
/// Thrown when the cache service task dropped.
#[derive(Debug, thiserror::Error)]
#[error("cache service task stopped")]
pub struct CacheServiceUnavailable;

impl From<CacheServiceUnavailable> for ProviderError {
    fn from(err: CacheServiceUnavailable) -> Self {
        Self::other(err)
    }
}

/// A task that manages caches for data required by the `eth` rpc implementation.
///
/// It provides a caching layer on top of the given
/// [`StateProvider`](reth_storage_api::StateProvider) and keeps data fetched via the provider in
/// memory in an LRU cache. If the requested data is missing in the cache it is fetched and inserted
/// into the cache afterwards. While fetching data from disk is sync, this service is async since
/// requests and data is shared via channels.
///
/// This type is an endless future that listens for incoming messages from the user facing
/// [`EthStateCache`] via a channel. If the requested data is not cached then it spawns a new task
/// that does the IO and sends the result back to it. This way the caching service only
/// handles messages and does LRU lookups and never blocking IO.
///
/// Caution: The channel for the data is _unbounded_ it is assumed that this is mainly used by the
/// `reth_rpc::EthApi` which is typically invoked by the RPC server, which already uses
/// permits to limit concurrent requests.
#[must_use = "Type does nothing unless spawned"]
pub(crate) struct EthStateCacheService<
    Provider,
    Tasks,
    LimitBlocks = ByLength,
    LimitReceipts = ByLength,
    LimitHeaders = ByLength,
    LimitBals = ByLength,
> where
    Provider: BlockReader + BalProvider,
    LimitBlocks: Limiter<B256, Arc<RecoveredBlock<Provider::Block>>>,
    LimitReceipts: Limiter<B256, Arc<Vec<Provider::Receipt>>>,
    LimitHeaders: Limiter<B256, Provider::Header>,
    LimitBals: Limiter<B256, CachedRevmBal>,
{
    /// The type used to lookup data from disk
    provider: Provider,
    /// The LRU cache for full blocks grouped by their block hash.
    full_block_cache: BlockLruCache<Provider::Block, LimitBlocks>,
    /// The LRU cache for block receipts grouped by the block hash.
    receipts_cache: ReceiptsLruCache<Provider::Receipt, LimitReceipts>,
    /// The LRU cache for headers.
    ///
    /// Headers are cached because they are required to populate the environment for execution
    /// (evm).
    headers_cache: HeaderLruCache<Provider::Header, LimitHeaders>,
    /// The LRU cache for revm BALs grouped by the block hash.
    bal_cache: BalLruCache<LimitBals>,
    /// Sender half of the action channel.
    action_tx: UnboundedSender<CacheAction<Provider::Block, Provider::Receipt>>,
    /// Receiver half of the action channel.
    action_rx: UnboundedReceiverStream<CacheAction<Provider::Block, Provider::Receipt>>,
    /// The type that's used to spawn tasks that do the actual work
    action_task_spawner: Tasks,
    /// Rate limiter for spawned fetch tasks.
    ///
    /// This restricts the max concurrent fetch tasks at the same time.
    rate_limiter: Arc<Semaphore>,
    /// LRU index mapping transaction hashes to their block hash and index within the block.
    tx_hash_index: LruMap<TxHash, (B256, usize), ByLength>,
}

impl<Provider> EthStateCacheService<Provider, Runtime>
where
    Provider: BlockReader + BalProvider + Clone + Unpin + 'static,
{
    /// Indexes all transactions in a block by transaction hash.
    fn index_block_transactions(&mut self, block: &RecoveredBlock<Provider::Block>) {
        let block_hash = block.hash();
        for (tx_idx, tx) in block.body().transactions().iter().enumerate() {
            self.tx_hash_index.insert(*tx.tx_hash(), (block_hash, tx_idx));
        }
    }

    /// Removes transaction index entries for a reorged block.
    fn remove_block_transactions(&mut self, block: &RecoveredBlock<Provider::Block>) {
        for tx in block.body().transactions() {
            self.tx_hash_index.remove(tx.tx_hash());
        }
    }

    fn on_new_block(
        &mut self,
        block_hash: B256,
        res: ProviderResult<Option<Arc<RecoveredBlock<Provider::Block>>>>,
    ) {
        if let Some(queued) = self.full_block_cache.remove(&block_hash) {
            // send the response to queued senders
            for tx in queued {
                let _ = tx.send(res.clone());
            }
        }

        // cache good block
        if let Ok(Some(block)) = res {
            self.full_block_cache.insert(block_hash, block);
        }
    }

    fn on_new_receipts(
        &mut self,
        block_hash: B256,
        res: ProviderResult<Option<Arc<Vec<Provider::Receipt>>>>,
    ) {
        if let Some(queued) = self.receipts_cache.remove(&block_hash) {
            // send the response to queued senders
            for tx in queued {
                let _ = tx.send(res.clone());
            }
        }

        // cache good receipts
        if let Ok(Some(receipts)) = res {
            self.receipts_cache.insert(block_hash, receipts);
        }
    }

    fn on_new_bal(&mut self, block_hash: B256, res: ProviderResult<Option<CachedRevmBal>>) {
        if let Some(queued) = self.bal_cache.remove(&block_hash) {
            for tx in queued {
                let _ = tx.send(res.clone());
            }
        }

        if let Ok(Some(bal)) = res {
            self.bal_cache.insert(block_hash, bal);
        }
    }

    fn on_reorg_block(
        &mut self,
        block_hash: B256,
        res: ProviderResult<Option<Arc<RecoveredBlock<Provider::Block>>>>,
    ) {
        if let Some(queued) = self.full_block_cache.remove(&block_hash) {
            // send the response to queued senders
            for tx in queued {
                let _ = tx.send(res.clone());
            }
        }
    }

    fn on_reorg_receipts(
        &mut self,
        block_hash: B256,
        res: ProviderResult<Option<Arc<Vec<Provider::Receipt>>>>,
    ) {
        if let Some(queued) = self.receipts_cache.remove(&block_hash) {
            // send the response to queued senders
            for tx in queued {
                let _ = tx.send(res.clone());
            }
        }
    }

    fn on_reorg_header(&mut self, block_hash: B256, res: ProviderResult<Provider::Header>) {
        if let Some(queued) = self.headers_cache.remove(&block_hash) {
            // send the response to queued senders
            for tx in queued {
                let _ = tx.send(res.clone());
            }
        }
    }

    fn on_reorg_bal(&mut self, block_hash: B256, res: ProviderResult<Option<CachedRevmBal>>) {
        if let Some(queued) = self.bal_cache.remove(&block_hash) {
            for tx in queued {
                let _ = tx.send(res.clone());
            }
        }
    }

    /// Shrinks the queues but leaves some space for the next requests
    fn shrink_queues(&mut self) {
        let min_capacity = 2;
        self.full_block_cache.shrink_to(min_capacity);
        self.receipts_cache.shrink_to(min_capacity);
        self.headers_cache.shrink_to(min_capacity);
        self.bal_cache.shrink_to(min_capacity);
    }

    fn update_cached_metrics(&self) {
        self.full_block_cache.update_cached_metrics();
        self.receipts_cache.update_cached_metrics();
        self.headers_cache.update_cached_metrics();
        self.bal_cache.update_cached_metrics();
    }
}

impl<Provider> Future for EthStateCacheService<Provider, Runtime>
where
    Provider: BlockReader + BalProvider + Clone + Unpin + 'static,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        loop {
            let Poll::Ready(action) = this.action_rx.poll_next_unpin(cx) else {
                // shrink queues if we don't have any work to do
                this.shrink_queues();
                return Poll::Pending;
            };

            match action {
                None => {
                    unreachable!("can't close")
                }
                Some(action) => {
                    match action {
                        CacheAction::GetCachedBlock { block_hash, response_tx } => {
                            let _ =
                                response_tx.send(this.full_block_cache.get(&block_hash).cloned());
                        }
                        CacheAction::GetCachedBlockAndReceipts { block_hash, response_tx } => {
                            let block = this.full_block_cache.get(&block_hash).cloned();
                            let receipts = this.receipts_cache.get(&block_hash).cloned();
                            let _ = response_tx.send((block, receipts));
                        }
                        CacheAction::GetBlockWithSenders { block_hash, response_tx } => {
                            if let Some(block) = this.full_block_cache.get(&block_hash).cloned() {
                                let _ = response_tx.send(Ok(Some(block)));
                                continue
                            }

                            // block is not in the cache, request it if this is the first consumer
                            if this.full_block_cache.queue(block_hash, response_tx) {
                                let provider = this.provider.clone();
                                let action_tx = this.action_tx.clone();
                                let rate_limiter = this.rate_limiter.clone();
                                let mut action_sender =
                                    ActionSender::new(CacheKind::Block, block_hash, action_tx);
                                this.action_task_spawner.spawn_blocking_task(async move {
                                    // Acquire permit
                                    let _permit = rate_limiter.acquire().await;
                                    // Only look in the database to prevent situations where we
                                    // looking up the tree is blocking
                                    let block_sender = provider
                                        .sealed_block_with_senders(
                                            BlockHashOrNumber::Hash(block_hash),
                                            TransactionVariant::WithHash,
                                        )
                                        .map(|maybe_block| maybe_block.map(Arc::new));
                                    action_sender.send_block(block_sender);
                                });
                            }
                        }
                        CacheAction::GetReceipts { block_hash, response_tx } => {
                            // check if block is cached
                            if let Some(receipts) = this.receipts_cache.get(&block_hash).cloned() {
                                let _ = response_tx.send(Ok(Some(receipts)));
                                continue
                            }

                            // block is not in the cache, request it if this is the first consumer
                            if this.receipts_cache.queue(block_hash, response_tx) {
                                let provider = this.provider.clone();
                                let action_tx = this.action_tx.clone();
                                let rate_limiter = this.rate_limiter.clone();
                                let mut action_sender =
                                    ActionSender::new(CacheKind::Receipt, block_hash, action_tx);
                                this.action_task_spawner.spawn_blocking_task(async move {
                                    // Acquire permit
                                    let _permit = rate_limiter.acquire().await;
                                    let res = provider
                                        .receipts_by_block(block_hash.into())
                                        .map(|maybe_receipts| maybe_receipts.map(Arc::new));

                                    action_sender.send_receipts(res);
                                });
                            }
                        }
                        CacheAction::GetHeader { block_hash, response_tx } => {
                            // check if the header is cached
                            if let Some(header) = this.headers_cache.get(&block_hash).cloned() {
                                let _ = response_tx.send(Ok(header));
                                continue
                            }

                            // it's possible we have the entire block cached
                            if let Some(block) = this.full_block_cache.get(&block_hash) {
                                let _ = response_tx.send(Ok(block.clone_header()));
                                continue
                            }

                            // header is not in the cache, request it if this is the first
                            // consumer
                            if this.headers_cache.queue(block_hash, response_tx) {
                                let provider = this.provider.clone();
                                let action_tx = this.action_tx.clone();
                                let rate_limiter = this.rate_limiter.clone();
                                let mut action_sender =
                                    ActionSender::new(CacheKind::Header, block_hash, action_tx);
                                this.action_task_spawner.spawn_blocking_task(async move {
                                    // Acquire permit
                                    let _permit = rate_limiter.acquire().await;
                                    let header = provider.header(block_hash).and_then(|header| {
                                        header.ok_or_else(|| {
                                            ProviderError::HeaderNotFound(block_hash.into())
                                        })
                                    });
                                    action_sender.send_header(header);
                                });
                            }
                        }
                        CacheAction::GetBal { block_hash, response_tx } => {
                            if let Some(bal) = this.bal_cache.get(&block_hash).cloned() {
                                let _ = response_tx.send(Ok(Some(bal)));
                                continue
                            }

                            if this.bal_cache.queue(block_hash, response_tx) {
                                let provider = this.provider.clone();
                                let action_tx = this.action_tx.clone();
                                let rate_limiter = this.rate_limiter.clone();
                                let mut action_sender =
                                    ActionSender::new(CacheKind::Bal, block_hash, action_tx);
                                this.action_task_spawner.spawn_blocking_task(async move {
                                    let _permit = rate_limiter.acquire().await;
                                    let res = provider
                                        .bal_store()
                                        .revm_bal_by_hash(block_hash)
                                        .map(|maybe_bal| maybe_bal.map(CachedRevmBal::new));
                                    action_sender.send_bal(res);
                                });
                            }
                        }
                        CacheAction::ReceiptsResult { block_hash, res } => {
                            this.on_new_receipts(block_hash, res);
                        }
                        CacheAction::BalResult { block_hash, res } => {
                            this.on_new_bal(block_hash, res);
                        }
                        CacheAction::BlockWithSendersResult { block_hash, res } => match res {
                            Ok(Some(block_with_senders)) => {
                                this.on_new_block(block_hash, Ok(Some(block_with_senders)));
                            }
                            Ok(None) => {
                                this.on_new_block(block_hash, Ok(None));
                            }
                            Err(e) => {
                                this.on_new_block(block_hash, Err(e));
                            }
                        },
                        CacheAction::HeaderResult { block_hash, res } => {
                            let res = *res;
                            if let Some(queued) = this.headers_cache.remove(&block_hash) {
                                // send the response to queued senders
                                for tx in queued {
                                    let _ = tx.send(res.clone());
                                }
                            }

                            // cache good header
                            if let Ok(data) = res {
                                this.headers_cache.insert(block_hash, data);
                            }
                        }
                        CacheAction::CacheNewCanonicalChain { chain_change } => {
                            for block in chain_change.blocks {
                                // Index transactions before caching the block
                                this.index_block_transactions(&block);
                                this.on_new_block(block.hash(), Ok(Some(block)));
                            }

                            for block_receipts in chain_change.receipts {
                                this.on_new_receipts(
                                    block_receipts.block_hash,
                                    Ok(Some(block_receipts.receipts)),
                                );
                            }
                        }
                        CacheAction::RemoveReorgedChain { chain_change } => {
                            for block in chain_change.blocks {
                                let block_hash = block.hash();
                                let header = block.clone_header();
                                // Remove transaction index entries for reorged blocks
                                this.remove_block_transactions(&block);
                                this.on_reorg_block(block_hash, Ok(Some(block)));
                                this.on_reorg_header(block_hash, Ok(header));
                                this.on_reorg_bal(block_hash, Ok(None));
                            }

                            for block_receipts in chain_change.receipts {
                                this.on_reorg_receipts(
                                    block_receipts.block_hash,
                                    Ok(Some(block_receipts.receipts)),
                                );
                            }
                        }
                        CacheAction::GetCachedParentBlocks {
                            block_hash,
                            max_blocks,
                            response_tx,
                        } => {
                            let mut blocks = Vec::new();
                            let mut current_hash = block_hash;

                            // Start with the requested block
                            while blocks.len() < max_blocks {
                                if let Some(block) =
                                    this.full_block_cache.get(&current_hash).cloned()
                                {
                                    // Get the parent hash for the next iteration
                                    current_hash = block.header().parent_hash();
                                    blocks.push(block);
                                } else {
                                    // Break the loop if we can't find the current block
                                    break;
                                }
                            }

                            let _ = response_tx.send(blocks);
                        }
                        CacheAction::GetTransactionByHash { tx_hash, response_tx } => {
                            let result =
                                this.tx_hash_index.get(&tx_hash).and_then(|(block_hash, idx)| {
                                    let block = this.full_block_cache.get(block_hash).cloned()?;
                                    let receipts = this.receipts_cache.get(block_hash).cloned();
                                    Some(CachedTransaction::new(block, *idx, receipts))
                                });
                            let _ = response_tx.send(result);
                        }
                    };
                    this.update_cached_metrics();
                }
            }
        }
    }
}

/// All message variants sent through the channel
enum CacheAction<B: Block, R> {
    GetBlockWithSenders {
        block_hash: B256,
        response_tx: BlockWithSendersResponseSender<B>,
    },
    GetHeader {
        block_hash: B256,
        response_tx: HeaderResponseSender<B::Header>,
    },
    GetReceipts {
        block_hash: B256,
        response_tx: ReceiptsResponseSender<R>,
    },
    GetBal {
        block_hash: B256,
        response_tx: BalResponseSender,
    },
    GetCachedBlock {
        block_hash: B256,
        response_tx: CachedBlockResponseSender<B>,
    },
    GetCachedBlockAndReceipts {
        block_hash: B256,
        response_tx: CachedBlockAndReceiptsResponseSender<B, R>,
    },
    BlockWithSendersResult {
        block_hash: B256,
        res: ProviderResult<Option<Arc<RecoveredBlock<B>>>>,
    },
    ReceiptsResult {
        block_hash: B256,
        res: ProviderResult<Option<Arc<Vec<R>>>>,
    },
    HeaderResult {
        block_hash: B256,
        res: Box<ProviderResult<B::Header>>,
    },
    BalResult {
        block_hash: B256,
        res: ProviderResult<Option<CachedRevmBal>>,
    },
    CacheNewCanonicalChain {
        chain_change: ChainChange<B, R>,
    },
    RemoveReorgedChain {
        chain_change: ChainChange<B, R>,
    },
    GetCachedParentBlocks {
        block_hash: B256,
        max_blocks: usize,
        response_tx: CachedParentBlocksResponseSender<B>,
    },
    /// Look up a transaction's cached data by its hash
    GetTransactionByHash {
        tx_hash: TxHash,
        response_tx: TransactionHashResponseSender<B, R>,
    },
}

struct BlockReceipts<R> {
    block_hash: B256,
    receipts: Arc<Vec<R>>,
}

/// A change of the canonical chain
struct ChainChange<B: Block, R> {
    blocks: Vec<Arc<RecoveredBlock<B>>>,
    receipts: Vec<BlockReceipts<R>>,
}

impl<B: Block, R: Clone> ChainChange<B, R> {
    fn new<N>(chain: Arc<Chain<N>>) -> Self
    where
        N: NodePrimitives<Block = B, Receipt = R>,
    {
        let (blocks, receipts): (Vec<_>, Vec<_>) = chain
            .blocks_and_receipts()
            .map(|(block, receipts)| {
                let block_receipts = BlockReceipts {
                    block_hash: block.hash(),
                    receipts: Arc::new(receipts.clone()),
                };
                (Arc::clone(block), block_receipts)
            })
            .unzip();
        Self { blocks, receipts }
    }
}

/// Identifier for the caches.
#[derive(Copy, Clone, Debug)]
enum CacheKind {
    Block,
    Receipt,
    Header,
    Bal,
}

/// Drop aware sender struct that ensures a response is always emitted even if the db task panics
/// before a result could be sent.
///
/// This type wraps a sender and in case the sender is still present on drop emit an error response.
#[derive(Debug)]
struct ActionSender<B: Block, R: Send + Sync> {
    kind: CacheKind,
    blockhash: B256,
    tx: Option<UnboundedSender<CacheAction<B, R>>>,
}

impl<R: Send + Sync, B: Block> ActionSender<B, R> {
    const fn new(kind: CacheKind, blockhash: B256, tx: UnboundedSender<CacheAction<B, R>>) -> Self {
        Self { kind, blockhash, tx: Some(tx) }
    }

    fn send_block(&mut self, block_sender: Result<Option<Arc<RecoveredBlock<B>>>, ProviderError>) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(CacheAction::BlockWithSendersResult {
                block_hash: self.blockhash,
                res: block_sender,
            });
        }
    }

    fn send_receipts(&mut self, receipts: Result<Option<Arc<Vec<R>>>, ProviderError>) {
        if let Some(tx) = self.tx.take() {
            let _ =
                tx.send(CacheAction::ReceiptsResult { block_hash: self.blockhash, res: receipts });
        }
    }

    fn send_header(&mut self, header: Result<<B as Block>::Header, ProviderError>) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(CacheAction::HeaderResult {
                block_hash: self.blockhash,
                res: Box::new(header),
            });
        }
    }

    fn send_bal(&mut self, bal: Result<Option<CachedRevmBal>, ProviderError>) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(CacheAction::BalResult { block_hash: self.blockhash, res: bal });
        }
    }
}
impl<R: Send + Sync, B: Block> Drop for ActionSender<B, R> {
    fn drop(&mut self) {
        if let Some(tx) = self.tx.take() {
            let msg = match self.kind {
                CacheKind::Block => CacheAction::BlockWithSendersResult {
                    block_hash: self.blockhash,
                    res: Err(CacheServiceUnavailable.into()),
                },
                CacheKind::Receipt => CacheAction::ReceiptsResult {
                    block_hash: self.blockhash,
                    res: Err(CacheServiceUnavailable.into()),
                },
                CacheKind::Header => CacheAction::HeaderResult {
                    block_hash: self.blockhash,
                    res: Box::new(Err(CacheServiceUnavailable.into())),
                },
                CacheKind::Bal => CacheAction::BalResult {
                    block_hash: self.blockhash,
                    res: Err(CacheServiceUnavailable.into()),
                },
            };
            let _ = tx.send(msg);
        }
    }
}

/// Awaits for new chain events and directly inserts them into the cache so they're available
/// immediately before they need to be fetched from disk.
///
/// Reorged blocks are removed from the cache.
pub async fn cache_new_blocks_task<St, N: NodePrimitives>(
    eth_state_cache: EthStateCache<N>,
    mut events: St,
) where
    St: Stream<Item = CanonStateNotification<N>> + Unpin + 'static,
{
    while let Some(event) = events.next().await {
        if let Some(reverted) = event.reverted() {
            let chain_change = ChainChange::new(reverted);

            let _ =
                eth_state_cache.to_service.send(CacheAction::RemoveReorgedChain { chain_change });
        }

        let chain_change = ChainChange::new(event.committed());

        let _ =
            eth_state_cache.to_service.send(CacheAction::CacheNewCanonicalChain { chain_change });
    }
}

/// Cached decoded revm BAL.
#[derive(Clone, Debug)]
pub(crate) struct CachedRevmBal(Arc<DecodedBal<Arc<RevmBal>>>);

impl CachedRevmBal {
    /// Creates a cached revm BAL from an owned decoded BAL.
    #[inline]
    fn new(bal: DecodedBal<Arc<RevmBal>>) -> Self {
        Self(Arc::new(bal))
    }
}

impl InMemorySize for CachedRevmBal {
    fn size(&self) -> usize {
        core::mem::size_of::<Self>() + decoded_revm_bal_size(&self.0)
    }
}

fn decoded_revm_bal_size(bal: &DecodedBal<Arc<RevmBal>>) -> usize {
    core::mem::size_of::<DecodedBal<Arc<RevmBal>>>() +
        bal.as_raw().len() +
        revm_bal_size(bal.as_bal())
}

fn revm_bal_size(bal: &Arc<RevmBal>) -> usize {
    core::mem::size_of::<RevmBal>() +
        bal.accounts.capacity() * core::mem::size_of::<(Address, RevmAccountBal)>() +
        bal.accounts.values().map(revm_account_bal_heap_size).sum::<usize>()
}

fn revm_account_bal_heap_size(account: &RevmAccountBal) -> usize {
    revm_account_info_bal_heap_size(&account.account_info) +
        revm_storage_bal_heap_size(&account.storage)
}

fn revm_account_info_bal_heap_size(account_info: &RevmAccountInfoBal) -> usize {
    revm_bal_writes_heap_size(&account_info.nonce, |_| 0) +
        revm_bal_writes_heap_size(&account_info.balance, |_| 0) +
        revm_bal_writes_heap_size(&account_info.code, revm_code_write_heap_size)
}

fn revm_storage_bal_heap_size(storage: &RevmStorageBal) -> usize {
    storage.storage.len() * core::mem::size_of::<(StorageKey, RevmBalWrites<StorageValue>)>() +
        storage
            .storage
            .values()
            .map(|writes| revm_bal_writes_heap_size(writes, |_| 0))
            .sum::<usize>()
}

fn revm_bal_writes_heap_size<T, F>(writes: &RevmBalWrites<T>, mut item_heap_size: F) -> usize
where
    T: PartialEq + Clone,
    F: FnMut(&T) -> usize,
{
    writes.writes.capacity() * core::mem::size_of::<(u64, T)>() +
        writes.writes.iter().map(|(_, item)| item_heap_size(item)).sum::<usize>()
}

fn revm_code_write_heap_size((_, bytecode): &(B256, Bytecode)) -> usize {
    bytecode.bytes_ref().len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{transaction::TransactionMeta, Header};
    use alloy_eip7928::BlockAccessIndex;
    use alloy_eips::{BlockHashOrNumber, NumHash};
    use alloy_primitives::{Address, BlockHash, BlockNumber, Bytes, Signature, TxHash, TxNumber};
    use core::ops::{RangeBounds, RangeInclusive};
    use reth_db_models::StoredBlockBodyIndices;
    use reth_ethereum_primitives::{
        Block, BlockBody, EthPrimitives, Receipt, Transaction, TransactionSigned,
    };
    use reth_primitives_traits::{RecoveredBlock, SealedHeader};
    use reth_storage_api::{
        noop::NoopProvider, BalProvider, BalStore, BalStoreHandle, BlockBodyIndicesProvider,
        BlockHashReader, BlockNumReader, BlockReader, BlockSource, HeaderProvider, ReceiptProvider,
        TransactionVariant, TransactionsProvider,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn test_service() -> EthStateCacheService<NoopProvider, Runtime> {
        let (_cache, service) = EthStateCache::<EthPrimitives>::create(
            NoopProvider::default(),
            Runtime::test(),
            EthStateCacheConfig {
                max_blocks: 4,
                max_receipts: 4,
                max_headers: 4,
                max_bals: 4,
                max_concurrent_db_requests: 1,
                max_cached_tx_hashes: 16,
            },
        );
        service
    }

    fn test_decoded_revm_bal() -> DecodedBal<Arc<RevmBal>> {
        DecodedBal::new(Arc::new(RevmBal::default()), Bytes::from_static(&[0xc0]))
    }

    fn test_block() -> RecoveredBlock<Block> {
        RecoveredBlock::new_unhashed(
            Block {
                header: Header { number: 1, ..Default::default() },
                body: BlockBody {
                    transactions: vec![TransactionSigned::new_unhashed(
                        Transaction::Legacy(Default::default()),
                        Signature::test_signature(),
                    )],
                    ..Default::default()
                },
            },
            vec![Address::ZERO],
        )
    }

    #[test]
    fn reorg_evicts_cached_headers() {
        let mut service = test_service();
        let block_hash = B256::repeat_byte(0x11);

        assert!(service
            .headers_cache
            .insert(block_hash, Header { number: 42, ..Default::default() }));
        assert!(service.headers_cache.get(&block_hash).is_some());

        service.on_reorg_header(block_hash, Ok(Header { number: 7, ..Default::default() }));

        assert!(service.headers_cache.get(&block_hash).is_none());
    }

    #[test]
    fn reorg_forwards_header_to_queued_requests() {
        let mut service = test_service();
        let block_hash = B256::repeat_byte(0x22);
        let (response_tx, mut response_rx) = oneshot::channel();
        let header = Header { number: 7, ..Default::default() };

        assert!(service.headers_cache.queue(block_hash, response_tx));

        service.on_reorg_header(block_hash, Ok(header));

        let header =
            response_rx.try_recv().expect("queued header response").expect("header result");

        assert_eq!(header.number, 7);
    }

    #[test]
    fn reorg_removes_tx_hash_index_entries_unconditionally() {
        let mut service = test_service();
        let block = test_block();
        let tx_hash = *block.body().transactions().next().expect("test transaction").tx_hash();

        service.tx_hash_index.insert(tx_hash, (B256::repeat_byte(0x33), 0));

        service.remove_block_transactions(&block);

        assert!(service.tx_hash_index.get(&tx_hash).is_none());
    }

    #[test]
    fn reorg_evicts_cached_bal() {
        let mut service = test_service();
        let block_hash = B256::repeat_byte(0x44);

        assert!(service.bal_cache.insert(block_hash, CachedRevmBal::new(test_decoded_revm_bal())));
        assert!(service.bal_cache.get(&block_hash).is_some());

        service.on_reorg_bal(block_hash, Ok(None));

        assert!(service.bal_cache.get(&block_hash).is_none());
    }

    #[test]
    fn reorg_forwards_bal_to_queued_requests() {
        let mut service = test_service();
        let block_hash = B256::repeat_byte(0x55);
        let (response_tx, mut response_rx) = oneshot::channel();
        let bal = CachedRevmBal::new(test_decoded_revm_bal());

        assert!(service.bal_cache.queue(block_hash, response_tx));

        service.on_reorg_bal(block_hash, Ok(Some(bal)));

        let bal = response_rx.try_recv().expect("queued BAL response").expect("BAL result");

        assert!(bal.is_some());
    }

    #[test]
    fn cached_revm_bal_size_accounts_for_nested_allocations() {
        let mut account = RevmAccountBal::default();
        account.account_info.nonce.writes.push((BlockAccessIndex::new(1), 1));
        account
            .account_info
            .balance
            .writes
            .push((BlockAccessIndex::new(2), StorageValue::from(1u64)));
        account.account_info.code.writes.push((
            BlockAccessIndex::new(3),
            (B256::repeat_byte(0xaa), Bytecode::new_raw(Bytes::from_static(&[0x60, 0x00]))),
        ));
        account.storage.storage.insert(
            StorageKey::from(1u64),
            RevmBalWrites::new(vec![(BlockAccessIndex::new(4), StorageValue::from(2u64))]),
        );

        let mut bal = RevmBal::default();
        bal.accounts.insert(Address::ZERO, account);

        let raw = Bytes::from_static(&[0xc0, 0x01, 0x02]);
        let previous_estimate = core::mem::size_of::<CachedRevmBal>() +
            core::mem::size_of::<DecodedBal<Arc<RevmBal>>>() +
            raw.len() +
            core::mem::size_of::<RevmBal>();
        assert!(CachedRevmBal::new(DecodedBal::new(Arc::new(bal), raw)).size() > previous_estimate);
    }

    #[tokio::test]
    async fn get_bal_uses_cached_revm_bal() {
        let fetches = Arc::new(AtomicUsize::default());
        let provider = TestBalProvider::new(fetches.clone());
        let cache = EthStateCache::<EthPrimitives>::spawn_with(
            provider,
            EthStateCacheConfig {
                max_blocks: 0,
                max_receipts: 0,
                max_headers: 0,
                max_bals: 4,
                max_concurrent_db_requests: 1,
                max_cached_tx_hashes: 0,
            },
            Runtime::test(),
        );
        let block_hash = B256::repeat_byte(0x66);

        assert!(cache.get_bal(block_hash).await.unwrap().is_some());
        assert!(cache.get_bal(block_hash).await.unwrap().is_some());

        assert_eq!(fetches.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn concurrent_get_bal_requests_share_fetch() {
        let fetches = Arc::new(AtomicUsize::default());
        let provider = TestBalProvider::new(fetches.clone());
        let cache = EthStateCache::<EthPrimitives>::spawn_with(
            provider,
            EthStateCacheConfig {
                max_blocks: 0,
                max_receipts: 0,
                max_headers: 0,
                max_bals: 4,
                max_concurrent_db_requests: 1,
                max_cached_tx_hashes: 0,
            },
            Runtime::test(),
        );
        let block_hash = B256::repeat_byte(0x77);

        let (first, second) = tokio::join!(cache.get_bal(block_hash), cache.get_bal(block_hash));

        assert!(first.unwrap().is_some());
        assert!(second.unwrap().is_some());
        assert_eq!(fetches.load(Ordering::SeqCst), 1);
    }

    #[derive(Clone, Debug, Default)]
    struct TestBalProvider {
        bal_store: BalStoreHandle,
    }

    impl TestBalProvider {
        fn new(fetches: Arc<AtomicUsize>) -> Self {
            Self { bal_store: BalStoreHandle::new(TestBalStore { fetches }) }
        }
    }

    impl BalProvider for TestBalProvider {
        fn bal_store(&self) -> &BalStoreHandle {
            &self.bal_store
        }
    }

    #[derive(Debug)]
    struct TestBalStore {
        fetches: Arc<AtomicUsize>,
    }

    impl BalStore for TestBalStore {
        fn insert(&self, _num_hash: NumHash, _bal: reth_storage_api::RawBal) -> ProviderResult<()> {
            Ok(())
        }

        fn prune(&self, _tip: BlockNumber) -> ProviderResult<usize> {
            Ok(0)
        }

        fn get_by_hashes(&self, block_hashes: &[BlockHash]) -> ProviderResult<Vec<Option<Bytes>>> {
            Ok(block_hashes.iter().map(|_| None).collect())
        }

        fn revm_bal_by_hash(
            &self,
            _block_hash: BlockHash,
        ) -> ProviderResult<Option<DecodedBal<Arc<RevmBal>>>> {
            self.fetches.fetch_add(1, Ordering::SeqCst);
            Ok(Some(test_decoded_revm_bal()))
        }

        fn bal_stream(&self) -> reth_storage_api::BalNotificationStream {
            reth_storage_api::NoopBalStore.bal_stream()
        }
    }

    impl BlockHashReader for TestBalProvider {
        fn block_hash(&self, _number: BlockNumber) -> ProviderResult<Option<B256>> {
            Ok(None)
        }

        fn canonical_hashes_range(
            &self,
            _start: BlockNumber,
            _end: BlockNumber,
        ) -> ProviderResult<Vec<B256>> {
            Ok(Vec::new())
        }
    }

    impl BlockNumReader for TestBalProvider {
        fn chain_info(&self) -> ProviderResult<reth_chainspec::ChainInfo> {
            Ok(reth_chainspec::ChainInfo::default())
        }

        fn best_block_number(&self) -> ProviderResult<BlockNumber> {
            Ok(0)
        }

        fn last_block_number(&self) -> ProviderResult<BlockNumber> {
            Ok(0)
        }

        fn block_number(&self, _hash: B256) -> ProviderResult<Option<BlockNumber>> {
            Ok(None)
        }
    }

    impl HeaderProvider for TestBalProvider {
        type Header = Header;

        fn header(&self, _block_hash: BlockHash) -> ProviderResult<Option<Self::Header>> {
            Ok(None)
        }

        fn header_by_number(&self, _num: u64) -> ProviderResult<Option<Self::Header>> {
            Ok(None)
        }

        fn headers_range(
            &self,
            _range: impl RangeBounds<BlockNumber>,
        ) -> ProviderResult<Vec<Self::Header>> {
            Ok(Vec::new())
        }

        fn sealed_header(
            &self,
            _number: BlockNumber,
        ) -> ProviderResult<Option<SealedHeader<Self::Header>>> {
            Ok(None)
        }

        fn sealed_headers_while(
            &self,
            _range: impl RangeBounds<BlockNumber>,
            _predicate: impl FnMut(&SealedHeader<Self::Header>) -> bool,
        ) -> ProviderResult<Vec<SealedHeader<Self::Header>>> {
            Ok(Vec::new())
        }
    }

    impl BlockBodyIndicesProvider for TestBalProvider {
        fn block_body_indices(&self, _num: u64) -> ProviderResult<Option<StoredBlockBodyIndices>> {
            Ok(None)
        }

        fn block_body_indices_range(
            &self,
            _range: RangeInclusive<BlockNumber>,
        ) -> ProviderResult<Vec<StoredBlockBodyIndices>> {
            Ok(Vec::new())
        }
    }

    impl TransactionsProvider for TestBalProvider {
        type Transaction = TransactionSigned;

        fn transaction_id(&self, _tx_hash: TxHash) -> ProviderResult<Option<TxNumber>> {
            Ok(None)
        }

        fn transaction_by_id(&self, _id: TxNumber) -> ProviderResult<Option<Self::Transaction>> {
            Ok(None)
        }

        fn transaction_by_id_unhashed(
            &self,
            _id: TxNumber,
        ) -> ProviderResult<Option<Self::Transaction>> {
            Ok(None)
        }

        fn transaction_by_hash(&self, _hash: TxHash) -> ProviderResult<Option<Self::Transaction>> {
            Ok(None)
        }

        fn transaction_by_hash_with_meta(
            &self,
            _hash: TxHash,
        ) -> ProviderResult<Option<(Self::Transaction, TransactionMeta)>> {
            Ok(None)
        }

        fn transactions_by_block(
            &self,
            _block: BlockHashOrNumber,
        ) -> ProviderResult<Option<Vec<Self::Transaction>>> {
            Ok(None)
        }

        fn transactions_by_block_range(
            &self,
            _range: impl RangeBounds<BlockNumber>,
        ) -> ProviderResult<Vec<Vec<Self::Transaction>>> {
            Ok(Vec::new())
        }

        fn transactions_by_tx_range(
            &self,
            _range: impl RangeBounds<TxNumber>,
        ) -> ProviderResult<Vec<Self::Transaction>> {
            Ok(Vec::new())
        }

        fn senders_by_tx_range(
            &self,
            _range: impl RangeBounds<TxNumber>,
        ) -> ProviderResult<Vec<Address>> {
            Ok(Vec::new())
        }

        fn transaction_sender(&self, _id: TxNumber) -> ProviderResult<Option<Address>> {
            Ok(None)
        }
    }

    impl ReceiptProvider for TestBalProvider {
        type Receipt = Receipt;

        fn receipt(&self, _id: TxNumber) -> ProviderResult<Option<Self::Receipt>> {
            Ok(None)
        }

        fn receipt_by_hash(&self, _hash: TxHash) -> ProviderResult<Option<Self::Receipt>> {
            Ok(None)
        }

        fn receipts_by_block(
            &self,
            _block: BlockHashOrNumber,
        ) -> ProviderResult<Option<Vec<Self::Receipt>>> {
            Ok(None)
        }

        fn receipts_by_tx_range(
            &self,
            _range: impl RangeBounds<TxNumber>,
        ) -> ProviderResult<Vec<Self::Receipt>> {
            Ok(Vec::new())
        }

        fn receipts_by_block_range(
            &self,
            _block_range: RangeInclusive<BlockNumber>,
        ) -> ProviderResult<Vec<Vec<Self::Receipt>>> {
            Ok(Vec::new())
        }
    }

    impl BlockReader for TestBalProvider {
        type Block = Block;

        fn find_block_by_hash(
            &self,
            _hash: B256,
            _source: BlockSource,
        ) -> ProviderResult<Option<Self::Block>> {
            Ok(None)
        }

        fn block(&self, _id: BlockHashOrNumber) -> ProviderResult<Option<Self::Block>> {
            Ok(None)
        }

        fn pending_block(&self) -> ProviderResult<Option<RecoveredBlock<Self::Block>>> {
            Ok(None)
        }

        fn pending_block_and_receipts(
            &self,
        ) -> ProviderResult<Option<(RecoveredBlock<Self::Block>, Vec<Self::Receipt>)>> {
            Ok(None)
        }

        fn recovered_block(
            &self,
            _id: BlockHashOrNumber,
            _transaction_kind: TransactionVariant,
        ) -> ProviderResult<Option<RecoveredBlock<Self::Block>>> {
            Ok(None)
        }

        fn sealed_block_with_senders(
            &self,
            _id: BlockHashOrNumber,
            _transaction_kind: TransactionVariant,
        ) -> ProviderResult<Option<RecoveredBlock<Self::Block>>> {
            Ok(None)
        }

        fn block_range(
            &self,
            _range: RangeInclusive<BlockNumber>,
        ) -> ProviderResult<Vec<Self::Block>> {
            Ok(Vec::new())
        }

        fn block_with_senders_range(
            &self,
            _range: RangeInclusive<BlockNumber>,
        ) -> ProviderResult<Vec<RecoveredBlock<Self::Block>>> {
            Ok(Vec::new())
        }

        fn recovered_block_range(
            &self,
            _range: RangeInclusive<BlockNumber>,
        ) -> ProviderResult<Vec<RecoveredBlock<Self::Block>>> {
            Ok(Vec::new())
        }

        fn block_by_transaction_id(&self, _id: TxNumber) -> ProviderResult<Option<BlockNumber>> {
            Ok(None)
        }
    }
}
