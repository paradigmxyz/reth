//! Async caching support for eth RPC

use reth_primitives::{Block, H256};
use reth_provider::{EvmEnvProvider, StateProviderFactory};
use reth_tasks::TaskSpawner;
use revm::primitives::{BlockEnv, CfgEnv};
use schnellru::{ByMemoryUsage, Limiter, LruMap};
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tokio_stream::wrappers::UnboundedReceiverStream;

/// Provides async access to cached eth data
///
/// This is the frontend to the [EthStateCacheService] which manages cached data on a different
/// task.
#[derive(Debug, Clone)]
pub struct EthStateCache {
    to_service: UnboundedSender<CacheAction>,
}

impl EthStateCache {
    fn create<Client>(
        client: Client,
        action_task_spawner: Box<dyn TaskSpawner>,
        max_block_bytes: usize,
        max_env_bytes: usize,
    ) -> (Self, EthStateCacheService<Client>) {
        let (to_service, rx) = unbounded_channel();
        let service = EthStateCacheService {
            client,
            full_block_cache: LruMap::with_memory_budget(max_block_bytes),
            evm_env_cache: LruMap::with_memory_budget(max_env_bytes),
            action_rx: UnboundedReceiverStream::new(rx),
            action_task_spawner,
        };
        let cache = EthStateCache { to_service };
        (cache, service)
    }

    /// Creates a new async LRU backed cache service task and spawns it to a new task via the given
    /// spawner.
    ///
    /// The cache is memory limited by the given max bytes values.
    pub fn spawn<Client>(
        client: Client,
        spawner: Box<dyn TaskSpawner>,
        max_block_bytes: usize,
        max_env_bytes: usize,
    ) -> Self
    where
        Client: StateProviderFactory + EvmEnvProvider + Clone + 'static,
    {
        let (this, service) = Self::create(client, spawner.clone(), max_block_bytes, max_env_bytes);
        spawner.spawn(Box::pin(service));
        this
    }
}

/// A task than manages caches for data required by the `eth` rpc implementation.
///
/// It provides a caching layer on top of the given [StateProvider] and keeps data fetched via the
/// provider in memory in an LRU cache. If the requested data is missing in the cache it is fetched
/// and inserted into the cache afterwards. While fetching data from disk is sync, this service is
/// async since requests and data is shared via channels.
///
/// This type is an endless future that listens for incoming messages from the user facing
/// [EthStateCache] via a channel. If the requested data is not cached then it spawns a new task
/// that does the IO and sends the result back to it. This way the [EthStateCacheService] only
/// handles messages and does LRU lookups and never blocking IO.
///
/// Caution: The channel for the data is _unbounded_ it is assumed that this is mainly used by the
/// [EthApi](crate::EthApi) which is typically invoked by the RPC server, which already uses permits
/// to limit concurrent requests.
#[must_use = "Type does nothing unless spawned"]
pub struct EthStateCacheService<Client, LimitBlocks = ByMemoryUsage, LimitEnvs = ByMemoryUsage>
where
    LimitBlocks: Limiter<H256, Block>,
    LimitEnvs: Limiter<H256, (CfgEnv, BlockEnv)>,
{
    ///
    client: Client,
    /// The LRU cache for full blocks grouped by their hash.
    full_block_cache: LruMap<H256, Block, LimitBlocks>,
    /// The LRU cache for revm environments
    evm_env_cache: LruMap<H256, (CfgEnv, BlockEnv), LimitEnvs>,
    /// Receiver half of the action channel.
    action_rx: UnboundedReceiverStream<CacheAction>,
    /// The type that's used to spawn tasks that do the actual work
    action_task_spawner: Box<dyn TaskSpawner>,
}

impl<Client, LimitBlocks, LimitEnvs> EthStateCacheService<Client, LimitBlocks, LimitEnvs>
where
    Client: StateProviderFactory + EvmEnvProvider + Clone + 'static,
    LimitBlocks: Limiter<H256, Block>,
    LimitEnvs: Limiter<H256, (CfgEnv, BlockEnv)>,
{
}

impl<Client, LimitBlocks, LimitEnvs> Future for EthStateCacheService<Client, LimitBlocks, LimitEnvs>
where
    Client: StateProviderFactory + EvmEnvProvider + Clone + 'static,
    LimitBlocks: Limiter<H256, Block>,
    LimitEnvs: Limiter<H256, (CfgEnv, BlockEnv)>,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        todo!()
    }
}

///
enum CacheAction {}
