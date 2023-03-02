//! Async caching support for eth RPC

use futures::StreamExt;
use reth_interfaces::{provider::ProviderError, Result};
use reth_primitives::{Block, H256};
use reth_provider::{BlockProvider, EvmEnvProvider, StateProviderFactory};
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

/// The type that can send the response to a requested [Block]
type BlockResponseSender = oneshot::Sender<Result<Option<Block>>>;

/// The type that can send the response to a requested env
type EnvResponseSender = oneshot::Sender<Result<(CfgEnv, BlockEnv)>>;

type BlockLruCache<L> = MultiConsumerLruCache<H256, Block, L, BlockResponseSender>;

type EnvLruCache<L> = MultiConsumerLruCache<H256, (CfgEnv, BlockEnv), L, EnvResponseSender>;

/// Settings for the [EthStateCache]
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EthStateCacheConfig {
    /// Max number of bytes for cached block data.
    ///
    /// Default is 50MB
    pub max_block_bytes: usize,
    /// Max number of bytes for cached env data.
    ///
    /// Default is 500kb (env configs are very small)
    pub max_env_bytes: usize,
}

impl Default for EthStateCacheConfig {
    fn default() -> Self {
        Self { max_block_bytes: 50 * 1024 * 1024, max_env_bytes: 500 * 1024 }
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
        max_env_bytes: usize,
    ) -> (Self, EthStateCacheService<Client, Tasks>) {
        let (to_service, rx) = unbounded_channel();
        let service = EthStateCacheService {
            client,
            full_block_cache: BlockLruCache::with_memory_budget(max_block_bytes),
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
        let EthStateCacheConfig { max_block_bytes, max_env_bytes } = config;
        let (this, service) =
            Self::create(client, executor.clone(), max_block_bytes, max_env_bytes);
        executor.spawn(Box::pin(service));
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

    /// Requests the evm env config for the block hash.
    ///
    /// Returns an error if the corresponding header (required for populating the envs) was not
    /// found.
    pub(crate) async fn get_evm_evn(&self, block_hash: H256) -> Result<(CfgEnv, BlockEnv)> {
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
    LimitEnvs = ByMemoryUsage,
> where
    LimitBlocks: Limiter<H256, Block>,
    LimitEnvs: Limiter<H256, (CfgEnv, BlockEnv)>,
{
    /// The type used to lookup data from disk
    client: Client,
    /// The LRU cache for full blocks grouped by their hash.
    full_block_cache: BlockLruCache<LimitBlocks>,
    /// The LRU cache for revm environments
    evm_env_cache: EnvLruCache<LimitEnvs>,
    /// Sender half of the action channel.
    action_tx: UnboundedSender<CacheAction>,
    /// Receiver half of the action channel.
    action_rx: UnboundedReceiverStream<CacheAction>,
    /// The type that's used to spawn tasks that do the actual work
    action_task_spawner: Tasks,
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
                            if this.full_block_cache.queue(block_hash, response_tx) {
                                let client = this.client.clone();
                                let action_tx = this.action_tx.clone();
                                this.action_task_spawner.spawn(Box::pin(async move {
                                    let res = client.block_by_hash(block_hash);
                                    let _ = action_tx
                                        .send(CacheAction::BlockResult { block_hash, res });
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
                                this.action_task_spawner.spawn(Box::pin(async move {
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
                            if let Some(queued) = this.full_block_cache.queued.remove(&block_hash) {
                                // send the response to queued senders
                                for tx in queued {
                                    let _ = tx.send(res.clone());
                                }
                            }

                            // cache good block
                            if let Ok(Some(block)) = res {
                                this.full_block_cache.cache.insert(block_hash, block);
                            }
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
    GetEnv { block_hash: H256, response_tx: EnvResponseSender },
    BlockResult { block_hash: H256, res: Result<Option<Block>> },
    EnvResult { block_hash: H256, res: Box<Result<(CfgEnv, BlockEnv)>> },
}
