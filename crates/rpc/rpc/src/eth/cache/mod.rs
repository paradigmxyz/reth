//! Async caching support for eth RPC

use futures::{future::Either, Stream, StreamExt};
use reth_interfaces::provider::{ProviderError, ProviderResult};
use reth_node_api::ConfigureEvmEnv;
use reth_primitives::{
    Block, BlockHashOrNumber, BlockWithSenders, Receipt, SealedBlock, SealedBlockWithSenders,
    TransactionSigned, TransactionSignedEcRecovered, B256,
};
use reth_provider::{
    BlockReader, CanonStateNotification, Chain, EvmEnvProvider, StateProviderFactory,
    TransactionVariant,
};
use reth_tasks::{TaskSpawner, TokioTaskExecutor};
use revm::primitives::{BlockEnv, CfgEnv, CfgEnvWithHandlerCfg, SpecId};
use schnellru::{ByLength, Limiter};
use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
};
use tokio::sync::{
    mpsc::{unbounded_channel, UnboundedSender},
    oneshot, Semaphore,
};
use tokio_stream::wrappers::UnboundedReceiverStream;

mod config;
pub use config::*;

mod metrics;

mod multi_consumer;
pub use multi_consumer::MultiConsumerLruCache;

/// The type that can send the response to a requested [Block]
type BlockTransactionsResponseSender =
    oneshot::Sender<ProviderResult<Option<Vec<TransactionSigned>>>>;

/// The type that can send the response to a requested [BlockWithSenders]
type BlockWithSendersResponseSender = oneshot::Sender<ProviderResult<Option<BlockWithSenders>>>;

/// The type that can send the response to the requested receipts of a block.
type ReceiptsResponseSender = oneshot::Sender<ProviderResult<Option<Arc<Vec<Receipt>>>>>;

/// The type that can send the response to a requested env
type EnvResponseSender = oneshot::Sender<ProviderResult<(CfgEnvWithHandlerCfg, BlockEnv)>>;

type BlockLruCache<L> = MultiConsumerLruCache<
    B256,
    BlockWithSenders,
    L,
    Either<BlockWithSendersResponseSender, BlockTransactionsResponseSender>,
>;

type ReceiptsLruCache<L> =
    MultiConsumerLruCache<B256, Arc<Vec<Receipt>>, L, ReceiptsResponseSender>;

type EnvLruCache<L> =
    MultiConsumerLruCache<B256, (CfgEnvWithHandlerCfg, BlockEnv), L, EnvResponseSender>;

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
    fn create<Provider, Tasks, EvmConfig>(
        provider: Provider,
        action_task_spawner: Tasks,
        evm_config: EvmConfig,
        max_blocks: u32,
        max_receipts: u32,
        max_envs: u32,
        max_concurrent_db_operations: usize,
    ) -> (Self, EthStateCacheService<Provider, Tasks, EvmConfig>) {
        let (to_service, rx) = unbounded_channel();
        let service = EthStateCacheService {
            provider,
            full_block_cache: BlockLruCache::new(max_blocks, "blocks"),
            receipts_cache: ReceiptsLruCache::new(max_receipts, "receipts"),
            evm_env_cache: EnvLruCache::new(max_envs, "evm_env"),
            action_tx: to_service.clone(),
            action_rx: UnboundedReceiverStream::new(rx),
            action_task_spawner,
            rate_limiter: Arc::new(Semaphore::new(max_concurrent_db_operations)),
            evm_config,
        };
        let cache = EthStateCache { to_service };
        (cache, service)
    }

    /// Creates a new async LRU backed cache service task and spawns it to a new task via
    /// [tokio::spawn].
    ///
    /// See also [Self::spawn_with]
    pub fn spawn<Provider, EvmConfig>(
        provider: Provider,
        config: EthStateCacheConfig,
        evm_config: EvmConfig,
    ) -> Self
    where
        Provider: StateProviderFactory + BlockReader + EvmEnvProvider + Clone + Unpin + 'static,
        EvmConfig: ConfigureEvmEnv + 'static,
    {
        Self::spawn_with(provider, config, TokioTaskExecutor::default(), evm_config)
    }

    /// Creates a new async LRU backed cache service task and spawns it to a new task via the given
    /// spawner.
    ///
    /// The cache is memory limited by the given max bytes values.
    pub fn spawn_with<Provider, Tasks, EvmConfig>(
        provider: Provider,
        config: EthStateCacheConfig,
        executor: Tasks,
        evm_config: EvmConfig,
    ) -> Self
    where
        Provider: StateProviderFactory + BlockReader + EvmEnvProvider + Clone + Unpin + 'static,
        Tasks: TaskSpawner + Clone + 'static,
        EvmConfig: ConfigureEvmEnv + 'static,
    {
        let EthStateCacheConfig { max_blocks, max_receipts, max_envs, max_concurrent_db_requests } =
            config;
        let (this, service) = Self::create(
            provider,
            executor.clone(),
            evm_config,
            max_blocks,
            max_receipts,
            max_envs,
            max_concurrent_db_requests,
        );
        executor.spawn_critical("eth state cache", Box::pin(service));
        this
    }

    /// Requests the [Block] for the block hash
    ///
    /// Returns `None` if the block does not exist.
    pub async fn get_block(&self, block_hash: B256) -> ProviderResult<Option<Block>> {
        let (response_tx, rx) = oneshot::channel();
        let _ = self.to_service.send(CacheAction::GetBlockWithSenders { block_hash, response_tx });
        let block_with_senders_res =
            rx.await.map_err(|_| ProviderError::CacheServiceUnavailable)?;

        if let Ok(Some(block_with_senders)) = block_with_senders_res {
            Ok(Some(block_with_senders.block))
        } else {
            Ok(None)
        }
    }

    /// Requests the [Block] for the block hash, sealed with the given block hash.
    ///
    /// Returns `None` if the block does not exist.
    pub async fn get_sealed_block(&self, block_hash: B256) -> ProviderResult<Option<SealedBlock>> {
        Ok(self.get_block(block_hash).await?.map(|block| block.seal(block_hash)))
    }

    /// Requests the transactions of the [Block]
    ///
    /// Returns `None` if the block does not exist.
    pub async fn get_block_transactions(
        &self,
        block_hash: B256,
    ) -> ProviderResult<Option<Vec<TransactionSigned>>> {
        let (response_tx, rx) = oneshot::channel();
        let _ = self.to_service.send(CacheAction::GetBlockTransactions { block_hash, response_tx });
        rx.await.map_err(|_| ProviderError::CacheServiceUnavailable)?
    }

    /// Requests the ecrecovered transactions of the [Block]
    ///
    /// Returns `None` if the block does not exist.
    pub async fn get_block_transactions_ecrecovered(
        &self,
        block_hash: B256,
    ) -> ProviderResult<Option<Vec<TransactionSignedEcRecovered>>> {
        Ok(self
            .get_block_with_senders(block_hash)
            .await?
            .map(|block| block.into_transactions_ecrecovered().collect()))
    }

    /// Fetches both transactions and receipts for the given block hash.
    pub async fn get_transactions_and_receipts(
        &self,
        block_hash: B256,
    ) -> ProviderResult<Option<(Vec<TransactionSigned>, Arc<Vec<Receipt>>)>> {
        let transactions = self.get_block_transactions(block_hash);
        let receipts = self.get_receipts(block_hash);

        let (transactions, receipts) = futures::try_join!(transactions, receipts)?;

        Ok(transactions.zip(receipts))
    }

    /// Requests the  [BlockWithSenders] for the block hash
    ///
    /// Returns `None` if the block does not exist.
    pub async fn get_block_with_senders(
        &self,
        block_hash: B256,
    ) -> ProviderResult<Option<BlockWithSenders>> {
        let (response_tx, rx) = oneshot::channel();
        let _ = self.to_service.send(CacheAction::GetBlockWithSenders { block_hash, response_tx });
        rx.await.map_err(|_| ProviderError::CacheServiceUnavailable)?
    }

    /// Requests the  [SealedBlockWithSenders] for the block hash
    ///
    /// Returns `None` if the block does not exist.
    pub async fn get_sealed_block_with_senders(
        &self,
        block_hash: B256,
    ) -> ProviderResult<Option<SealedBlockWithSenders>> {
        Ok(self.get_block_with_senders(block_hash).await?.map(|block| block.seal(block_hash)))
    }

    /// Requests the [Receipt] for the block hash
    ///
    /// Returns `None` if the block was not found.
    pub async fn get_receipts(
        &self,
        block_hash: B256,
    ) -> ProviderResult<Option<Arc<Vec<Receipt>>>> {
        let (response_tx, rx) = oneshot::channel();
        let _ = self.to_service.send(CacheAction::GetReceipts { block_hash, response_tx });
        rx.await.map_err(|_| ProviderError::CacheServiceUnavailable)?
    }

    /// Fetches both receipts and block for the given block hash.
    pub async fn get_block_and_receipts(
        &self,
        block_hash: B256,
    ) -> ProviderResult<Option<(SealedBlock, Arc<Vec<Receipt>>)>> {
        let block = self.get_sealed_block(block_hash);
        let receipts = self.get_receipts(block_hash);

        let (block, receipts) = futures::try_join!(block, receipts)?;

        Ok(block.zip(receipts))
    }

    /// Requests the evm env config for the block hash.
    ///
    /// Returns an error if the corresponding header (required for populating the envs) was not
    /// found.
    pub async fn get_evm_env(
        &self,
        block_hash: B256,
    ) -> ProviderResult<(CfgEnvWithHandlerCfg, BlockEnv)> {
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
    EvmConfig,
    LimitBlocks = ByLength,
    LimitReceipts = ByLength,
    LimitEnvs = ByLength,
> where
    LimitBlocks: Limiter<B256, BlockWithSenders>,
    LimitReceipts: Limiter<B256, Arc<Vec<Receipt>>>,
    LimitEnvs: Limiter<B256, (CfgEnvWithHandlerCfg, BlockEnv)>,
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
    /// Rate limiter
    rate_limiter: Arc<Semaphore>,
    /// The type that determines how to configure the EVM.
    evm_config: EvmConfig,
}

impl<Provider, Tasks, EvmConfig> EthStateCacheService<Provider, Tasks, EvmConfig>
where
    Provider: StateProviderFactory + BlockReader + EvmEnvProvider + Clone + Unpin + 'static,
    Tasks: TaskSpawner + Clone + 'static,
    EvmConfig: ConfigureEvmEnv + 'static,
{
    fn on_new_block(&mut self, block_hash: B256, res: ProviderResult<Option<BlockWithSenders>>) {
        if let Some(queued) = self.full_block_cache.remove(&block_hash) {
            // send the response to queued senders
            for tx in queued {
                match tx {
                    Either::Left(block_with_senders) => {
                        let _ = block_with_senders.send(res.clone());
                    }
                    Either::Right(transaction_tx) => {
                        let _ = transaction_tx.send(
                            res.clone()
                                .map(|maybe_block| maybe_block.map(|block| block.block.body)),
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

    fn on_new_receipts(
        &mut self,
        block_hash: B256,
        res: ProviderResult<Option<Arc<Vec<Receipt>>>>,
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

    fn on_reorg_block(&mut self, block_hash: B256, res: ProviderResult<Option<BlockWithSenders>>) {
        if let Some(queued) = self.full_block_cache.remove(&block_hash) {
            // send the response to queued senders
            for tx in queued {
                match tx {
                    Either::Left(block_with_senders) => {
                        let _ = block_with_senders.send(res.clone());
                    }
                    Either::Right(transaction_tx) => {
                        let _ = transaction_tx.send(
                            res.clone()
                                .map(|maybe_block| maybe_block.map(|block| block.block.body)),
                        );
                    }
                }
            }
        }
    }

    fn on_reorg_receipts(
        &mut self,
        block_hash: B256,
        res: ProviderResult<Option<Arc<Vec<Receipt>>>>,
    ) {
        if let Some(queued) = self.receipts_cache.remove(&block_hash) {
            // send the response to queued senders
            for tx in queued {
                let _ = tx.send(res.clone());
            }
        }
    }

    fn update_cached_metrics(&self) {
        self.full_block_cache.update_cached_metrics();
        self.receipts_cache.update_cached_metrics();
        self.evm_env_cache.update_cached_metrics();
    }
}

impl<Provider, Tasks, EvmConfig> Future for EthStateCacheService<Provider, Tasks, EvmConfig>
where
    Provider: StateProviderFactory + BlockReader + EvmEnvProvider + Clone + Unpin + 'static,
    Tasks: TaskSpawner + Clone + 'static,
    EvmConfig: ConfigureEvmEnv + 'static,
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
                        CacheAction::GetBlockWithSenders { block_hash, response_tx } => {
                            if let Some(block) = this.full_block_cache.get(&block_hash).cloned() {
                                let _ = response_tx.send(Ok(Some(block)));
                                continue
                            }

                            // block is not in the cache, request it if this is the first consumer
                            if this.full_block_cache.queue(block_hash, Either::Left(response_tx)) {
                                let provider = this.provider.clone();
                                let action_tx = this.action_tx.clone();
                                let rate_limiter = this.rate_limiter.clone();
                                this.action_task_spawner.spawn_blocking(Box::pin(async move {
                                    // Acquire permit
                                    let _permit = rate_limiter.acquire().await;
                                    // Only look in the database to prevent situations where we
                                    // looking up the tree is blocking
                                    let block_sender = provider.block_with_senders(
                                        BlockHashOrNumber::Hash(block_hash),
                                        TransactionVariant::WithHash,
                                    );
                                    let _ = action_tx.send(CacheAction::BlockWithSendersResult {
                                        block_hash,
                                        res: block_sender,
                                    });
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
                                let rate_limiter = this.rate_limiter.clone();
                                this.action_task_spawner.spawn_blocking(Box::pin(async move {
                                    // Acquire permit
                                    let _permit = rate_limiter.acquire().await;
                                    // Only look in the database to prevent situations where we
                                    // looking up the tree is blocking
                                    let res = provider.block_with_senders(
                                        BlockHashOrNumber::Hash(block_hash),
                                        TransactionVariant::WithHash,
                                    );
                                    let _ = action_tx.send(CacheAction::BlockWithSendersResult {
                                        block_hash,
                                        res,
                                    });
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
                                let rate_limiter = this.rate_limiter.clone();
                                this.action_task_spawner.spawn_blocking(Box::pin(async move {
                                    // Acquire permit
                                    let _permit = rate_limiter.acquire().await;
                                    let res = provider
                                        .receipts_by_block(block_hash.into())
                                        .map(|maybe_receipts| maybe_receipts.map(Arc::new));

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
                                let rate_limiter = this.rate_limiter.clone();
                                let evm_config = this.evm_config.clone();
                                this.action_task_spawner.spawn_blocking(Box::pin(async move {
                                    // Acquire permit
                                    let _permit = rate_limiter.acquire().await;
                                    let mut cfg = CfgEnvWithHandlerCfg::new_with_spec_id(
                                        CfgEnv::default(),
                                        SpecId::LATEST,
                                    );
                                    let mut block_env = BlockEnv::default();
                                    let res = provider
                                        .fill_env_at(
                                            &mut cfg,
                                            &mut block_env,
                                            block_hash.into(),
                                            evm_config,
                                        )
                                        .map(|_| (cfg, block_env));
                                    let _ = action_tx.send(CacheAction::EnvResult {
                                        block_hash,
                                        res: Box::new(res),
                                    });
                                }));
                            }
                        }
                        CacheAction::ReceiptsResult { block_hash, res } => {
                            this.on_new_receipts(block_hash, res);
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
                        CacheAction::CacheNewCanonicalChain { chain_change } => {
                            for block in chain_change.blocks {
                                this.on_new_block(block.hash(), Ok(Some(block.unseal())));
                            }

                            for block_receipts in chain_change.receipts {
                                this.on_new_receipts(
                                    block_receipts.block_hash,
                                    Ok(Some(Arc::new(
                                        block_receipts.receipts.into_iter().flatten().collect(),
                                    ))),
                                );
                            }
                        }
                        CacheAction::RemoveReorgedChain { chain_change } => {
                            for block in chain_change.blocks {
                                this.on_reorg_block(block.hash(), Ok(Some(block.unseal())));
                            }

                            for block_receipts in chain_change.receipts {
                                this.on_reorg_receipts(
                                    block_receipts.block_hash,
                                    Ok(Some(Arc::new(
                                        block_receipts.receipts.into_iter().flatten().collect(),
                                    ))),
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
    GetBlockWithSenders { block_hash: B256, response_tx: BlockWithSendersResponseSender },
    GetBlockTransactions { block_hash: B256, response_tx: BlockTransactionsResponseSender },
    GetEnv { block_hash: B256, response_tx: EnvResponseSender },
    GetReceipts { block_hash: B256, response_tx: ReceiptsResponseSender },
    BlockWithSendersResult { block_hash: B256, res: ProviderResult<Option<BlockWithSenders>> },
    ReceiptsResult { block_hash: B256, res: ProviderResult<Option<Arc<Vec<Receipt>>>> },
    EnvResult { block_hash: B256, res: Box<ProviderResult<(CfgEnvWithHandlerCfg, BlockEnv)>> },
    CacheNewCanonicalChain { chain_change: ChainChange },
    RemoveReorgedChain { chain_change: ChainChange },
}

struct BlockReceipts {
    block_hash: B256,
    receipts: Vec<Option<Receipt>>,
}

/// A change of the canonical chain
struct ChainChange {
    blocks: Vec<SealedBlockWithSenders>,
    receipts: Vec<BlockReceipts>,
}

impl ChainChange {
    fn new(chain: Arc<Chain>) -> Self {
        let (blocks, receipts): (Vec<_>, Vec<_>) = chain
            .blocks_and_receipts()
            .map(|(block, receipts)| {
                let block_receipts =
                    BlockReceipts { block_hash: block.block.hash(), receipts: receipts.clone() };
                (block.clone(), block_receipts)
            })
            .unzip();
        Self { blocks, receipts }
    }
}

/// Awaits for new chain events and directly inserts them into the cache so they're available
/// immediately before they need to be fetched from disk.
///
/// Reorged blocks are removed from the cache.
pub async fn cache_new_blocks_task<St>(eth_state_cache: EthStateCache, mut events: St)
where
    St: Stream<Item = CanonStateNotification> + Unpin + 'static,
{
    while let Some(event) = events.next().await {
        if let Some(reverted) = event.reverted() {
            let chain_change = ChainChange::new(reverted);

            let _ =
                eth_state_cache.to_service.send(CacheAction::RemoveReorgedChain { chain_change });
        }

        if let Some(committed) = event.committed() {
            let chain_change = ChainChange::new(committed);

            let _ = eth_state_cache
                .to_service
                .send(CacheAction::CacheNewCanonicalChain { chain_change });
        }
    }
}
