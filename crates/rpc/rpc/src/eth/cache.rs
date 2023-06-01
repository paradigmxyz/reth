//! Async caching support for eth RPC

use futures::{future::Either, Stream, StreamExt};
use reth_interfaces::{provider::ProviderError, Result};
use reth_primitives::{Block, Receipt, SealedBlock, TransactionSigned, H256};
use reth_provider::{BlockProvider, CanonStateNotification, EvmEnvProvider, StateProviderFactory};
use reth_tasks::{TaskSpawner, TokioTaskExecutor};
use revm::primitives::{BlockEnv, CfgEnv};
use schnellru::{ByMemoryUsage, Limiter, LruMap};
use serde::{Deserialize, Serialize};
use std::{
    collections::{hash_map::Entry, HashMap},
    future::Future,
    hash::Hash,
    pin::Pin,
    task::{ready, Context, Poll},
};
use tokio::sync::{
    mpsc::{unbounded_channel, UnboundedSender},
    oneshot,
};
use tokio_stream::wrappers::UnboundedReceiverStream;

/// Default cache size for the block cache: 500MB
///
/// With an average block size of ~100kb this should be able to cache ~5000 blocks.
pub const DEFAULT_BLOCK_CACHE_SIZE_BYTES_MB: usize = 500;

/// Default cache size for the receipts cache: 500MB
pub const DEFAULT_RECEIPT_CACHE_SIZE_BYTES_MB: usize = 500;

/// Default cache size for the env cache: 1MB
pub const DEFAULT_ENV_CACHE_SIZE_BYTES_MB: usize = 1;

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

/// Settings for the [EthStateCache]
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EthStateCacheConfig {
    /// Max number of bytes for cached block data.
    ///
    /// Default is 500MB
    pub max_block_bytes: usize,
    /// Max number of bytes for cached receipt data.
    ///
    /// Default is 500MB
    pub max_receipt_bytes: usize,
    /// Max number of bytes for cached env data.
    ///
    /// Default is 1MB (env configs are very small)
    pub max_env_bytes: usize,
}

impl Default for EthStateCacheConfig {
    fn default() -> Self {
        Self {
            max_block_bytes: DEFAULT_BLOCK_CACHE_SIZE_BYTES_MB * 1024 * 1024,
            max_receipt_bytes: DEFAULT_RECEIPT_CACHE_SIZE_BYTES_MB * 1024 * 1024,
            max_env_bytes: DEFAULT_ENV_CACHE_SIZE_BYTES_MB * 1024 * 1024,
        }
    }
}

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
    fn create<Client, Tasks>(
        client: Client,
        action_task_spawner: Tasks,
        max_block_bytes: usize,
        max_receipt_bytes: usize,
        max_env_bytes: usize,
    ) -> (Self, EthStateCacheService<Client, Tasks>) {
        let (to_service, rx) = unbounded_channel();
        let service = EthStateCacheService {
            client,
            full_block_cache: BlockLruCache::with_memory_budget(max_block_bytes),
            receipts_cache: ReceiptsLruCache::with_memory_budget(max_receipt_bytes),
            evm_env_cache: EnvLruCache::with_memory_budget(max_env_bytes),
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
    pub fn spawn<Client>(client: Client, config: EthStateCacheConfig) -> Self
    where
        Client: StateProviderFactory + BlockProvider + EvmEnvProvider + Clone + Unpin + 'static,
    {
        Self::spawn_with(client, config, TokioTaskExecutor::default())
    }

    /// Creates a new async LRU backed cache service task and spawns it to a new task via the given
    /// spawner.
    ///
    /// The cache is memory limited by the given max bytes values.
    pub fn spawn_with<Client, Tasks>(
        client: Client,
        config: EthStateCacheConfig,
        executor: Tasks,
    ) -> Self
    where
        Client: StateProviderFactory + BlockProvider + EvmEnvProvider + Clone + Unpin + 'static,
        Tasks: TaskSpawner + Clone + 'static,
    {
        let EthStateCacheConfig { max_block_bytes, max_receipt_bytes, max_env_bytes } = config;
        let (this, service) = Self::create(
            client,
            executor.clone(),
            max_block_bytes,
            max_receipt_bytes,
            max_env_bytes,
        );
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

    /// Requests the [Receipt] for the block hash
    ///
    /// Returns `None` if the block was not found.
    pub(crate) async fn get_receipts(&self, block_hash: H256) -> Result<Option<Vec<Receipt>>> {
        let (response_tx, rx) = oneshot::channel();
        let _ = self.to_service.send(CacheAction::GetReceipts { block_hash, response_tx });
        rx.await.map_err(|_| ProviderError::CacheServiceUnavailable)?
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
    Client,
    Tasks,
    LimitBlocks = ByMemoryUsage,
    LimitReceipts = ByMemoryUsage,
    LimitEnvs = ByMemoryUsage,
> where
    LimitBlocks: Limiter<H256, Block>,
    LimitReceipts: Limiter<H256, Vec<Receipt>>,
    LimitEnvs: Limiter<H256, (CfgEnv, BlockEnv)>,
{
    /// The type used to lookup data from disk
    client: Client,
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

impl<Client, Tasks> EthStateCacheService<Client, Tasks>
where
    Client: StateProviderFactory + BlockProvider + EvmEnvProvider + Clone + Unpin + 'static,
    Tasks: TaskSpawner + Clone + 'static,
{
    fn on_new_block(&mut self, block_hash: H256, res: Result<Option<Block>>) {
        if let Some(queued) = self.full_block_cache.queued.remove(&block_hash) {
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
            self.full_block_cache.cache.insert(block_hash, block);
        }
    }

    fn on_new_receipts(&mut self, block_hash: H256, res: Result<Option<Vec<Receipt>>>) {
        if let Some(queued) = self.receipts_cache.queued.remove(&block_hash) {
            // send the response to queued senders
            for tx in queued {
                let _ = tx.send(res.clone());
            }
        }

        // cache good receipts
        if let Ok(Some(receipts)) = res {
            self.receipts_cache.cache.insert(block_hash, receipts);
        }
    }
}

impl<Client, Tasks> Future for EthStateCacheService<Client, Tasks>
where
    Client: StateProviderFactory + BlockProvider + EvmEnvProvider + Clone + Unpin + 'static,
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
                            if let Some(block) =
                                this.full_block_cache.cache.get(&block_hash).cloned()
                            {
                                let _ = response_tx.send(Ok(Some(block)));
                                continue
                            }

                            // block is not in the cache, request it if this is the first consumer
                            if this.full_block_cache.queue(block_hash, Either::Left(response_tx)) {
                                let client = this.client.clone();
                                let action_tx = this.action_tx.clone();
                                this.action_task_spawner.spawn_blocking(Box::pin(async move {
                                    let res = client.block_by_hash(block_hash);
                                    let _ = action_tx
                                        .send(CacheAction::BlockResult { block_hash, res });
                                }));
                            }
                        }
                        CacheAction::GetBlockTransactions { block_hash, response_tx } => {
                            // check if block is cached
                            if let Some(block) = this.full_block_cache.cache.get(&block_hash) {
                                let _ = response_tx.send(Ok(Some(block.body.clone())));
                                continue
                            }

                            // block is not in the cache, request it if this is the first consumer
                            if this.full_block_cache.queue(block_hash, Either::Right(response_tx)) {
                                let client = this.client.clone();
                                let action_tx = this.action_tx.clone();
                                this.action_task_spawner.spawn_blocking(Box::pin(async move {
                                    let res = client.block_by_hash(block_hash);
                                    let _ = action_tx
                                        .send(CacheAction::BlockResult { block_hash, res });
                                }));
                            }
                        }
                        CacheAction::GetReceipts { block_hash, response_tx } => {
                            // check if block is cached
                            if let Some(receipts) =
                                this.receipts_cache.cache.get(&block_hash).cloned()
                            {
                                let _ = response_tx.send(Ok(Some(receipts)));
                                continue
                            }

                            // block is not in the cache, request it if this is the first consumer
                            if this.receipts_cache.queue(block_hash, response_tx) {
                                let client = this.client.clone();
                                let action_tx = this.action_tx.clone();
                                this.action_task_spawner.spawn_blocking(Box::pin(async move {
                                    let res = client.receipts_by_block(block_hash.into());
                                    let _ = action_tx
                                        .send(CacheAction::ReceiptsResult { block_hash, res });
                                }));
                            }
                        }
                        CacheAction::GetEnv { block_hash, response_tx } => {
                            // check if env data is cached
                            if let Some(env) = this.evm_env_cache.cache.get(&block_hash).cloned() {
                                let _ = response_tx.send(Ok(env));
                                continue
                            }

                            // env data is not in the cache, request it if this is the first
                            // consumer
                            if this.evm_env_cache.queue(block_hash, response_tx) {
                                let client = this.client.clone();
                                let action_tx = this.action_tx.clone();
                                this.action_task_spawner.spawn_blocking(Box::pin(async move {
                                    let mut cfg = CfgEnv::default();
                                    let mut block_env = BlockEnv::default();
                                    let res = client
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
                            if let Some(queued) = this.evm_env_cache.queued.remove(&block_hash) {
                                // send the response to queued senders
                                for tx in queued {
                                    let _ = tx.send(res.clone());
                                }
                            }

                            // cache good env data
                            if let Ok(data) = res {
                                this.evm_env_cache.cache.insert(block_hash, data);
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
                    }
                }
            }
        }
    }
}

struct MultiConsumerLruCache<K, V, L, S>
where
    K: Hash + Eq,
    L: Limiter<K, V>,
{
    /// The LRU cache for the
    cache: LruMap<K, V, L>,
    /// All queued consumers
    queued: HashMap<K, Vec<S>>,
}

impl<K, V, L, S> MultiConsumerLruCache<K, V, L, S>
where
    K: Hash + Eq,
    L: Limiter<K, V>,
{
    /// Adds the sender to the queue for the given key.
    ///
    /// Returns true if this is the first queued sender for the key
    fn queue(&mut self, key: K, sender: S) -> bool {
        match self.queued.entry(key) {
            Entry::Occupied(mut entry) => {
                entry.get_mut().push(sender);
                false
            }
            Entry::Vacant(entry) => {
                entry.insert(vec![sender]);
                true
            }
        }
    }
}

impl<K, V, S> MultiConsumerLruCache<K, V, ByMemoryUsage, S>
where
    K: Hash + Eq,
{
    /// Creates a new empty map with a given `memory_budget`.
    ///
    /// See also [LruMap::with_memory_budget]
    fn with_memory_budget(memory_budget: usize) -> Self {
        Self { cache: LruMap::with_memory_budget(memory_budget), queued: Default::default() }
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

            let mut receipts = Vec::new();

            // TODO ideally we can map all receipts to their respective blocks

            // we only have 1 block in the new chain, so we know all the receipts belong to the
            // block
            if blocks.len() == 1 {
                let block_receipts = BlockReceipts {
                    block_hash: blocks[0].hash,
                    receipts: state.receipts(blocks[0].number).to_vec(),
                };
                receipts.push(block_receipts);
            }
            let _ = eth_state_cache
                .to_service
                .send(CacheAction::CacheNewCanonicalChain { blocks, receipts });
        }
    }
}
