//! Async caching support for eth RPC

use futures::{future::Either, Stream, StreamExt};
use reth_interfaces::{provider::ProviderError, Result};
use reth_primitives::{Block, Receipt, SealedBlock, TransactionSigned, H256};
use reth_provider::{
    BlockReader, BlockSource, CanonStateNotification, EvmEnvProvider, StateProviderFactory,
};
use reth_tasks::{TaskSpawner, TokioTaskExecutor};
use revm::primitives::{BlockEnv, CfgEnv};
use schnellru::{ByLength, Limiter};
use std::{
    future::Future,
    pin::Pin,
    task::{ready, Context, Poll},
};
use tokio::sync::{
    mpsc::{unbounded_channel, UnboundedSender},
    oneshot,
};
use tokio_stream::wrappers::UnboundedReceiverStream;

mod config;
pub use config::*;

mod metrics;

mod multi_consumer;
pub use multi_consumer::MultiConsumerLruCache;

/// The type that can send the response to a requested [Block]
type BlockResponseSender = oneshot::Sender<Result<Option<Block>>>;

/// The type that can send the response to a requested [Block]
type BlockTransactionsResponseSender = oneshot::Sender<Result<Option<Vec<TransactionSigned>>>>;

/// The type that can send the response to the requested receipts of a block.
type ReceiptsResponseSender = oneshot::Sender<Result<Option<Vec<Receipt>>>>;

/// The type that can send the response to a requested env
type EnvResponseSender = oneshot::Sender<Result<(CfgEnv, BlockEnv)>>;

type BlockLruCache<L> = MultiConsumerLruCache<
    H256,
    Block,
    L,
    Either<BlockResponseSender, BlockTransactionsResponseSender>,
>;

type ReceiptsLruCache<L> = MultiConsumerLruCache<H256, Vec<Receipt>, L, ReceiptsResponseSender>;

type EnvLruCache<L> = MultiConsumerLruCache<H256, (CfgEnv, BlockEnv), L, EnvResponseSender>;

/// Provides async access to cached eth data
///
/// This is the frontend for the async caching service which manages cached data on a different
/// task.
#[derive(Debug, Clone)]
pub struct EthStateCache {
    to_service: UnboundedSender<CacheAction>,
}

impl EthStateCache {
    /// Creates and returns both [EthStateCache] frontend and the memory bound service.
    fn create<Provider, Tasks>(
        provider: Provider,
        action_task_spawner: Tasks,
        max_blocks: u32,
        max_receipts: u32,
        max_envs: u32,
    ) -> (Self, EthStateCacheService<Provider, Tasks>) {
        let (to_service, rx) = unbounded_channel();
        let service = EthStateCacheService {
            provider,
            full_block_cache: BlockLruCache::new(max_blocks, "blocks"),
            receipts_cache: ReceiptsLruCache::new(max_receipts, "receipts"),
            evm_env_cache: EnvLruCache::new(max_envs, "evm_env"),
            action_tx: to_service.clone(),
            action_rx: UnboundedReceiverStream::new(rx),
            action_task_spawner,
        };
        let cache = EthStateCache { to_service };
        (cache, service)
    }

    /// Creates a new async LRU backed cache service task and spawns it to a new task via
    /// [tokio::spawn].
    ///
    /// See also [Self::spawn_with]
    pub fn spawn<Provider>(provider: Provider, config: EthStateCacheConfig) -> Self
    where
        Provider: StateProviderFactory + BlockReader + EvmEnvProvider + Clone + Unpin + 'static,
    {
        Self::spawn_with(provider, config, TokioTaskExecutor::default())
    }

    /// Creates a new async LRU backed cache service task and spawns it to a new task via the given
    /// spawner.
    ///
    /// The cache is memory limited by the given max bytes values.
    pub fn spawn_with<Provider, Tasks>(
        provider: Provider,
        config: EthStateCacheConfig,
        executor: Tasks,
    ) -> Self
    where
        Provider: StateProviderFactory + BlockReader + EvmEnvProvider + Clone + Unpin + 'static,
        Tasks: TaskSpawner + Clone + 'static,
    {
        let EthStateCacheConfig { max_blocks, max_receipts, max_envs } = config;
        let (this, service) =
            Self::create(provider, executor.clone(), max_blocks, max_receipts, max_envs);
        executor.spawn_critical("eth state cache", Box::pin(service));
        this
    }

    /// Requests the [Block] for the block hash
    ///
    /// Returns `None` if the block does not exist.
    pub(crate) async fn get_block(&self, block_hash: H256) -> Result<Option<Block>> {
        let (response_tx, rx) = oneshot::channel();
        let _ = self.to_service.send(CacheAction::GetBlock { block_hash, response_tx });
        rx.await.map_err(|_| ProviderError::CacheServiceUnavailable)?
    }

    /// Requests the [Block] for the block hash, sealed with the given block hash.
    ///
    /// Returns `None` if the block does not exist.
    pub(crate) async fn get_sealed_block(&self, block_hash: H256) -> Result<Option<SealedBlock>> {
        Ok(self.get_block(block_hash).await?.map(|block| block.seal(block_hash)))
    }

    /// Requests the transactions of the [Block]
    ///
    /// Returns `None` if the block does not exist.
    pub(crate) async fn get_block_transactions(
        &self,
        block_hash: H256,
    ) -> Result<Option<Vec<TransactionSigned>>> {
        let (response_tx, rx) = oneshot::channel();
        let _ = self.to_service.send(CacheAction::GetBlockTransactions { block_hash, response_tx });
        rx.await.map_err(|_| ProviderError::CacheServiceUnavailable)?
    }

    /// Fetches both transactions and receipts for the given block hash.
    pub(crate) async fn get_transactions_and_receipts(
        &self,
        block_hash: H256,
    ) -> Result<Option<(Vec<TransactionSigned>, Vec<Receipt>)>> {
        let transactions = self.get_block_transactions(block_hash);
        let receipts = self.get_receipts(block_hash);

        let (transactions, receipts) = futures::try_join!(transactions, receipts)?;

        Ok(transactions.zip(receipts))
    }

    /// Requests the [Receipt] for the block hash
    ///
    /// Returns `None` if the block was not found.
    pub(crate) async fn get_receipts(&self, block_hash: H256) -> Result<Option<Vec<Receipt>>> {
        let (response_tx, rx) = oneshot::channel();
        let _ = self.to_service.send(CacheAction::GetReceipts { block_hash, response_tx });
        rx.await.map_err(|_| ProviderError::CacheServiceUnavailable)?
    }

    /// Fetches both receipts and block for the given block hash.
    pub(crate) async fn get_block_and_receipts(
        &self,
        block_hash: H256,
    ) -> Result<Option<(SealedBlock, Vec<Receipt>)>> {
        let block = self.get_sealed_block(block_hash);
        let receipts = self.get_receipts(block_hash);

        let (block, receipts) = futures::try_join!(block, receipts)?;

        Ok(block.zip(receipts))
    }

    /// Requests the evm env config for the block hash.
    ///
    /// Returns an error if the corresponding header (required for populating the envs) was not
    /// found.
    pub(crate) async fn get_evm_env(&self, block_hash: H256) -> Result<(CfgEnv, BlockEnv)> {
        let (response_tx, rx) = oneshot::channel();
        let _ = self.to_service.send(CacheAction::GetEnv { block_hash, response_tx });
        rx.await.map_err(|_| ProviderError::CacheServiceUnavailable)?
    }
}

/// A task than manages caches for data required by the `eth` rpc implementation.
///
/// It provides a caching layer on top of the given [StateProvider](reth_provider::StateProvider)
/// and keeps data fetched via the provider in memory in an LRU cache. If the requested data is
/// missing in the cache it is fetched and inserted into the cache afterwards. While fetching data
/// from disk is sync, this service is async since requests and data is shared via channels.
///
/// This type is an endless future that listens for incoming messages from the user facing
/// [EthStateCache] via a channel. If the requested data is not cached then it spawns a new task
/// that does the IO and sends the result back to it. This way the caching service only
/// handles messages and does LRU lookups and never blocking IO.
///
/// Caution: The channel for the data is _unbounded_ it is assumed that this is mainly used by the
/// [EthApi](crate::EthApi) which is typically invoked by the RPC server, which already uses permits
/// to limit concurrent requests.
#[must_use = "Type does nothing unless spawned"]
pub(crate) struct EthStateCacheService<
    Provider,
    Tasks,
    LimitBlocks = ByLength,
    LimitReceipts = ByLength,
    LimitEnvs = ByLength,
> where
    LimitBlocks: Limiter<H256, Block>,
    LimitReceipts: Limiter<H256, Vec<Receipt>>,
    LimitEnvs: Limiter<H256, (CfgEnv, BlockEnv)>,
{
    /// The type used to lookup data from disk
    provider: Provider,
    /// The LRU cache for full blocks grouped by their hash.
    full_block_cache: BlockLruCache<LimitBlocks>,
    /// The LRU cache for full blocks grouped by their hash.
    receipts_cache: ReceiptsLruCache<LimitReceipts>,
    /// The LRU cache for revm environments
    evm_env_cache: EnvLruCache<LimitEnvs>,
    /// Sender half of the action channel.
    action_tx: UnboundedSender<CacheAction>,
    /// Receiver half of the action channel.
    action_rx: UnboundedReceiverStream<CacheAction>,
    /// The type that's used to spawn tasks that do the actual work
    action_task_spawner: Tasks,
}

impl<Provider, Tasks> EthStateCacheService<Provider, Tasks>
where
    Provider: StateProviderFactory + BlockReader + EvmEnvProvider + Clone + Unpin + 'static,
    Tasks: TaskSpawner + Clone + 'static,
{
    fn on_new_block(&mut self, block_hash: H256, res: Result<Option<Block>>) {
        if let Some(queued) = self.full_block_cache.remove(&block_hash) {
            // send the response to queued senders
            for tx in queued {
                match tx {
                    Either::Left(block_tx) => {
                        let _ = block_tx.send(res.clone());
                    }
                    Either::Right(transaction_tx) => {
                        let _ = transaction_tx.send(
                            res.clone().map(|maybe_block| maybe_block.map(|block| block.body)),
                        );
                    }
                }
            }
        }

        // cache good block
        if let Ok(Some(block)) = res {
            self.full_block_cache.insert(block_hash, block);
        }
    }

    fn on_new_receipts(&mut self, block_hash: H256, res: Result<Option<Vec<Receipt>>>) {
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

    fn update_cached_metrics(&self) {
        self.full_block_cache.update_cached_metrics();
        self.receipts_cache.update_cached_metrics();
        self.evm_env_cache.update_cached_metrics();
    }
}

impl<Provider, Tasks> Future for EthStateCacheService<Provider, Tasks>
where
    Provider: StateProviderFactory + BlockReader + EvmEnvProvider + Clone + Unpin + 'static,
    Tasks: TaskSpawner + Clone + 'static,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        loop {
            match ready!(this.action_rx.poll_next_unpin(cx)) {
                None => {
                    unreachable!("can't close")
                }
                Some(action) => {
                    match action {
                        CacheAction::GetBlock { block_hash, response_tx } => {
                            // check if block is cached
                            if let Some(block) = this.full_block_cache.get(&block_hash).cloned() {
                                let _ = response_tx.send(Ok(Some(block)));
                                continue
                            }

                            // block is not in the cache, request it if this is the first consumer
                            if this.full_block_cache.queue(block_hash, Either::Left(response_tx)) {
                                let provider = this.provider.clone();
                                let action_tx = this.action_tx.clone();
                                this.action_task_spawner.spawn_blocking(Box::pin(async move {
                                    // Only look in the database to prevent situations where we
                                    // looking up the tree is blocking
                                    let res = provider
                                        .find_block_by_hash(block_hash, BlockSource::Database);
                                    let _ = action_tx
                                        .send(CacheAction::BlockResult { block_hash, res });
                                }));
                            }
                        }
                        CacheAction::GetBlockTransactions { block_hash, response_tx } => {
                            // check if block is cached
                            if let Some(block) = this.full_block_cache.get(&block_hash) {
                                let _ = response_tx.send(Ok(Some(block.body.clone())));
                                continue
                            }

                            // block is not in the cache, request it if this is the first consumer
                            if this.full_block_cache.queue(block_hash, Either::Right(response_tx)) {
                                let provider = this.provider.clone();
                                let action_tx = this.action_tx.clone();
                                this.action_task_spawner.spawn_blocking(Box::pin(async move {
                                    // Only look in the database to prevent situations where we
                                    // looking up the tree is blocking
                                    let res = provider
                                        .find_block_by_hash(block_hash, BlockSource::Database);
                                    let _ = action_tx
                                        .send(CacheAction::BlockResult { block_hash, res });
                                }));
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
                                this.action_task_spawner.spawn_blocking(Box::pin(async move {
                                    let res = provider.receipts_by_block(block_hash.into());
                                    let _ = action_tx
                                        .send(CacheAction::ReceiptsResult { block_hash, res });
                                }));
                            }
                        }
                        CacheAction::GetEnv { block_hash, response_tx } => {
                            // check if env data is cached
                            if let Some(env) = this.evm_env_cache.get(&block_hash).cloned() {
                                let _ = response_tx.send(Ok(env));
                                continue
                            }

                            // env data is not in the cache, request it if this is the first
                            // consumer
                            if this.evm_env_cache.queue(block_hash, response_tx) {
                                let provider = this.provider.clone();
                                let action_tx = this.action_tx.clone();
                                this.action_task_spawner.spawn_blocking(Box::pin(async move {
                                    let mut cfg = CfgEnv::default();
                                    let mut block_env = BlockEnv::default();
                                    let res = provider
                                        .fill_env_at(&mut cfg, &mut block_env, block_hash.into())
                                        .map(|_| (cfg, block_env));
                                    let _ = action_tx.send(CacheAction::EnvResult {
                                        block_hash,
                                        res: Box::new(res),
                                    });
                                }));
                            }
                        }
                        CacheAction::BlockResult { block_hash, res } => {
                            this.on_new_block(block_hash, res);
                        }
                        CacheAction::ReceiptsResult { block_hash, res } => {
                            this.on_new_receipts(block_hash, res);
                        }
                        CacheAction::EnvResult { block_hash, res } => {
                            let res = *res;
                            if let Some(queued) = this.evm_env_cache.remove(&block_hash) {
                                // send the response to queued senders
                                for tx in queued {
                                    let _ = tx.send(res.clone());
                                }
                            }

                            // cache good env data
                            if let Ok(data) = res {
                                this.evm_env_cache.insert(block_hash, data);
                            }
                        }
                        CacheAction::CacheNewCanonicalChain { blocks, receipts } => {
                            for block in blocks {
                                this.on_new_block(block.hash, Ok(Some(block.unseal())));
                            }

                            for block_receipts in receipts {
                                this.on_new_receipts(
                                    block_receipts.block_hash,
                                    Ok(Some(block_receipts.receipts)),
                                );
                            }
                        }
                    };
                    this.update_cached_metrics();
                }
            }
        }
    }
}

/// All message variants sent through the channel
enum CacheAction {
    GetBlock { block_hash: H256, response_tx: BlockResponseSender },
    GetBlockTransactions { block_hash: H256, response_tx: BlockTransactionsResponseSender },
    GetEnv { block_hash: H256, response_tx: EnvResponseSender },
    GetReceipts { block_hash: H256, response_tx: ReceiptsResponseSender },
    BlockResult { block_hash: H256, res: Result<Option<Block>> },
    ReceiptsResult { block_hash: H256, res: Result<Option<Vec<Receipt>>> },
    EnvResult { block_hash: H256, res: Box<Result<(CfgEnv, BlockEnv)>> },
    CacheNewCanonicalChain { blocks: Vec<SealedBlock>, receipts: Vec<BlockReceipts> },
}

struct BlockReceipts {
    block_hash: H256,
    receipts: Vec<Receipt>,
}

/// Awaits for new chain events and directly inserts them into the cache so they're available
/// immediately before they need to be fetched from disk.
pub async fn cache_new_blocks_task<St>(eth_state_cache: EthStateCache, mut events: St)
where
    St: Stream<Item = CanonStateNotification> + Unpin + 'static,
{
    while let Some(event) = events.next().await {
        if let Some(committed) = event.committed() {
            // we're only interested in new committed blocks
            let (blocks, state) = committed.inner();

            let blocks = blocks.iter().map(|(_, block)| block.block.clone()).collect::<Vec<_>>();

            // also cache all receipts of the blocks
            let mut receipts = Vec::with_capacity(blocks.len());
            for block in &blocks {
                let block_receipts = BlockReceipts {
                    block_hash: block.hash,
                    receipts: state.receipts(block.number).to_vec(),
                };
                receipts.push(block_receipts);
            }

            let _ = eth_state_cache
                .to_service
                .send(CacheAction::CacheNewCanonicalChain { blocks, receipts });
        }
    }
}
