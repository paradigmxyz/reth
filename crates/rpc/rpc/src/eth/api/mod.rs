//! The entire implementation of the namespace is quite large, hence it is divided across several
//! files.

use crate::eth::{
    api::pending_block::{PendingBlock, PendingBlockEnv, PendingBlockEnvOrigin},
    cache::EthStateCache,
    error::{EthApiError, EthResult},
    gas_oracle::GasPriceOracle,
    signer::EthSigner,
};

use async_trait::async_trait;
use metrics::atomics::AtomicU64;
use reth_interfaces::RethResult;
use reth_network_api::NetworkInfo;
use reth_primitives::{
    revm_primitives::{BlockEnv, CfgEnv},
    Address, BlockId, BlockNumberOrTag, ChainInfo, Receipt, SealedBlock, SealedBlockWithSenders,
    TransactionSigned, B256, U256, U64,
};
use serde::{Deserialize, Serialize};

use reth_provider::{
    BlockReaderIdExt, CanonStateNotification, ChainSpecProvider, EvmEnvProvider, StateProviderBox,
    StateProviderFactory,
};
use reth_rpc_types::{SyncInfo, SyncStatus, TxGasAndReward};
use reth_tasks::{TaskSpawner, TokioTaskExecutor};
use reth_transaction_pool::TransactionPool;
use std::{
    collections::BTreeMap,
    fmt::Debug,
    future::Future,
    sync::{atomic::Ordering::SeqCst, Arc},
    time::{Duration, Instant},
};

use futures::{Stream, StreamExt};
use tokio::sync::{oneshot, Mutex};

mod block;
mod call;
mod fees;
#[cfg(feature = "optimism")]
mod optimism;
mod pending_block;
mod server;
mod sign;
mod state;
mod transactions;

use crate::BlockingTaskPool;
pub use transactions::{EthTransactions, TransactionSource};

/// `Eth` API trait.
///
/// Defines core functionality of the `eth` API implementation.
#[async_trait]
pub trait EthApiSpec: EthTransactions + Send + Sync {
    /// Returns the current ethereum protocol version.
    async fn protocol_version(&self) -> RethResult<U64>;

    /// Returns the chain id
    fn chain_id(&self) -> U64;

    /// Returns provider chain info
    fn chain_info(&self) -> RethResult<ChainInfo>;

    /// Returns a list of addresses owned by provider.
    fn accounts(&self) -> Vec<Address>;

    /// Returns `true` if the network is undergoing sync.
    fn is_syncing(&self) -> bool;

    /// Returns the [SyncStatus] of the network
    fn sync_status(&self) -> RethResult<SyncStatus>;
}

/// `Eth` API implementation.
///
/// This type provides the functionality for handling `eth_` related requests.
/// These are implemented two-fold: Core functionality is implemented as [EthApiSpec]
/// trait. Additionally, the required server implementations (e.g. [`reth_rpc_api::EthApiServer`])
/// are implemented separately in submodules. The rpc handler implementation can then delegate to
/// the main impls. This way [`EthApi`] is not limited to [`jsonrpsee`] and can be used standalone
/// or in other network handlers (for example ipc).
pub struct EthApi<Provider, Pool, Network> {
    /// All nested fields bundled together.
    inner: Arc<EthApiInner<Provider, Pool, Network>>,
}

impl<Provider, Pool, Network> EthApi<Provider, Pool, Network>
where
    Provider: BlockReaderIdExt + ChainSpecProvider,
{
    /// Creates a new, shareable instance using the default tokio task spawner.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        provider: Provider,
        pool: Pool,
        network: Network,
        eth_cache: EthStateCache,
        gas_oracle: GasPriceOracle<Provider>,
        gas_cap: impl Into<GasCap>,
        blocking_task_pool: BlockingTaskPool,
        fee_history_cache: FeeHistoryCache,
    ) -> Self {
        Self::with_spawner(
            provider,
            pool,
            network,
            eth_cache,
            gas_oracle,
            gas_cap.into().into(),
            Box::<TokioTaskExecutor>::default(),
            blocking_task_pool,
            fee_history_cache,
        )
    }

    /// Creates a new, shareable instance.
    #[allow(clippy::too_many_arguments)]
    pub fn with_spawner(
        provider: Provider,
        pool: Pool,
        network: Network,
        eth_cache: EthStateCache,
        gas_oracle: GasPriceOracle<Provider>,
        gas_cap: u64,
        task_spawner: Box<dyn TaskSpawner>,
        blocking_task_pool: BlockingTaskPool,
        fee_history_cache: FeeHistoryCache,
    ) -> Self {
        // get the block number of the latest block
        let latest_block = provider
            .header_by_number_or_tag(BlockNumberOrTag::Latest)
            .ok()
            .flatten()
            .map(|header| header.number)
            .unwrap_or_default();

        let inner = EthApiInner {
            provider,
            pool,
            network,
            signers: Default::default(),
            eth_cache,
            gas_oracle,
            gas_cap,
            starting_block: U256::from(latest_block),
            task_spawner,
            pending_block: Default::default(),
            blocking_task_pool,
            fee_history_cache,
            #[cfg(feature = "optimism")]
            http_client: reqwest::Client::new(),
        };

        Self { inner: Arc::new(inner) }
    }

    /// Executes the future on a new blocking task.
    ///
    /// This accepts a closure that creates a new future using a clone of this type and spawns the
    /// future onto a new task that is allowed to block.
    pub(crate) async fn on_blocking_task<C, F, R>(&self, c: C) -> EthResult<R>
    where
        C: FnOnce(Self) -> F,
        F: Future<Output = EthResult<R>> + Send + 'static,
        R: Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        let this = self.clone();
        let f = c(this);
        self.inner.task_spawner.spawn_blocking(Box::pin(async move {
            let res = f.await;
            let _ = tx.send(res);
        }));
        rx.await.map_err(|_| EthApiError::InternalEthError)?
    }

    /// Returns the state cache frontend
    pub(crate) fn cache(&self) -> &EthStateCache {
        &self.inner.eth_cache
    }

    /// Returns the gas oracle frontend
    pub(crate) fn gas_oracle(&self) -> &GasPriceOracle<Provider> {
        &self.inner.gas_oracle
    }

    /// Returns the configured gas limit cap for `eth_call` and tracing related calls
    pub fn gas_cap(&self) -> u64 {
        self.inner.gas_cap
    }

    /// Returns the inner `Provider`
    pub fn provider(&self) -> &Provider {
        &self.inner.provider
    }

    /// Returns the inner `Network`
    pub fn network(&self) -> &Network {
        &self.inner.network
    }

    /// Returns the inner `Pool`
    pub fn pool(&self) -> &Pool {
        &self.inner.pool
    }

    /// Returns fee history cache
    pub fn fee_history_cache(&self) -> &FeeHistoryCache {
        &self.inner.fee_history_cache
    }
}

// === State access helpers ===

impl<Provider, Pool, Network> EthApi<Provider, Pool, Network>
where
    Provider:
        BlockReaderIdExt + ChainSpecProvider + StateProviderFactory + EvmEnvProvider + 'static,
{
    /// Returns the state at the given [BlockId] enum.
    ///
    /// Note: if not [BlockNumberOrTag::Pending] then this will only return canonical state. See also <https://github.com/paradigmxyz/reth/issues/4515>
    pub fn state_at_block_id(&self, at: BlockId) -> EthResult<StateProviderBox> {
        Ok(self.provider().state_by_block_id(at)?)
    }

    /// Returns the state at the given [BlockId] enum or the latest.
    ///
    /// Convenience function to interprets `None` as `BlockId::Number(BlockNumberOrTag::Latest)`
    pub fn state_at_block_id_or_latest(
        &self,
        block_id: Option<BlockId>,
    ) -> EthResult<StateProviderBox> {
        if let Some(block_id) = block_id {
            self.state_at_block_id(block_id)
        } else {
            Ok(self.latest_state()?)
        }
    }

    /// Returns the state at the given block number
    pub fn state_at_hash(&self, block_hash: B256) -> RethResult<StateProviderBox> {
        Ok(self.provider().history_by_block_hash(block_hash)?)
    }

    /// Returns the _latest_ state
    pub fn latest_state(&self) -> RethResult<StateProviderBox> {
        Ok(self.provider().latest()?)
    }
}

impl<Provider, Pool, Network> EthApi<Provider, Pool, Network>
where
    Provider:
        BlockReaderIdExt + ChainSpecProvider + StateProviderFactory + EvmEnvProvider + 'static,
    Pool: TransactionPool + Clone + 'static,
    Network: NetworkInfo + Send + Sync + 'static,
{
    /// Configures the [CfgEnv] and [BlockEnv] for the pending block
    ///
    /// If no pending block is available, this will derive it from the `latest` block
    pub(crate) fn pending_block_env_and_cfg(&self) -> EthResult<PendingBlockEnv> {
        let origin = if let Some(pending) = self.provider().pending_block_with_senders()? {
            PendingBlockEnvOrigin::ActualPending(pending)
        } else {
            // no pending block from the CL yet, so we use the latest block and modify the env
            // values that we can
            let mut latest =
                self.provider().latest_header()?.ok_or_else(|| EthApiError::UnknownBlockNumber)?;

            // child block
            latest.number += 1;
            // assumed child block is in the next slot
            latest.timestamp += 12;
            // base fee of the child block
            let chain_spec = self.provider().chain_spec();
            latest.base_fee_per_gas = latest.next_block_base_fee(chain_spec.base_fee_params);

            PendingBlockEnvOrigin::DerivedFromLatest(latest)
        };

        let mut cfg = CfgEnv::default();

        #[cfg(feature = "optimism")]
        {
            cfg.optimism = self.provider().chain_spec().is_optimism();
        }

        let mut block_env = BlockEnv::default();
        // Note: for the PENDING block we assume it is past the known merge block and thus this will
        // not fail when looking up the total difficulty value for the blockenv.
        self.provider().fill_env_with_header(&mut cfg, &mut block_env, origin.header())?;

        Ok(PendingBlockEnv { cfg, block_env, origin })
    }

    /// Returns the locally built pending block
    pub(crate) async fn local_pending_block(&self) -> EthResult<Option<SealedBlockWithSenders>> {
        let pending = self.pending_block_env_and_cfg()?;
        if pending.origin.is_actual_pending() {
            return Ok(pending.origin.into_actual_pending());
        }

        // no pending block from the CL yet, so we need to build it ourselves via txpool
        self.on_blocking_task(|this| async move {
            let mut lock = this.inner.pending_block.lock().await;
            let now = Instant::now();

            // check if the block is still good
            if let Some(pending_block) = lock.as_ref() {
                // this is guaranteed to be the `latest` header
                if pending.block_env.number.to::<u64>() == pending_block.block.number
                    && pending.origin.header().hash == pending_block.block.parent_hash
                    && now <= pending_block.expires_at
                {
                    return Ok(Some(pending_block.block.clone()));
                }
            }

            // if we're currently syncing, we're unable to build a pending block
            if this.network().is_syncing() {
                return Ok(None);
            }

            // we rebuild the block
            let pending_block = match pending.build_block(this.provider(), this.pool()) {
                Ok(block) => block,
                Err(err) => {
                    tracing::debug!(target: "rpc", "Failed to build pending block: {:?}", err);
                    return Ok(None);
                }
            };

            let now = Instant::now();
            *lock = Some(PendingBlock {
                block: pending_block.clone(),
                expires_at: now + Duration::from_secs(3),
            });

            Ok(Some(pending_block))
        })
        .await
    }
}

impl<Provider, Pool, Events> std::fmt::Debug for EthApi<Provider, Pool, Events> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EthApi").finish_non_exhaustive()
    }
}

impl<Provider, Pool, Events> Clone for EthApi<Provider, Pool, Events> {
    fn clone(&self) -> Self {
        Self { inner: Arc::clone(&self.inner) }
    }
}

#[async_trait]
impl<Provider, Pool, Network> EthApiSpec for EthApi<Provider, Pool, Network>
where
    Pool: TransactionPool + Clone + 'static,
    Provider:
        BlockReaderIdExt + ChainSpecProvider + StateProviderFactory + EvmEnvProvider + 'static,
    Network: NetworkInfo + 'static,
{
    /// Returns the current ethereum protocol version.
    ///
    /// Note: This returns an `U64`, since this should return as hex string.
    async fn protocol_version(&self) -> RethResult<U64> {
        let status = self.network().network_status().await?;
        Ok(U64::from(status.protocol_version))
    }

    /// Returns the chain id
    fn chain_id(&self) -> U64 {
        U64::from(self.network().chain_id())
    }

    /// Returns the current info for the chain
    fn chain_info(&self) -> RethResult<ChainInfo> {
        Ok(self.provider().chain_info()?)
    }

    fn accounts(&self) -> Vec<Address> {
        self.inner.signers.iter().flat_map(|s| s.accounts()).collect()
    }

    fn is_syncing(&self) -> bool {
        self.network().is_syncing()
    }

    /// Returns the [SyncStatus] of the network
    fn sync_status(&self) -> RethResult<SyncStatus> {
        let status = if self.is_syncing() {
            let current_block = U256::from(
                self.provider().chain_info().map(|info| info.best_number).unwrap_or_default(),
            );
            SyncStatus::Info(SyncInfo {
                starting_block: self.inner.starting_block,
                current_block,
                highest_block: current_block,
                warp_chunks_amount: None,
                warp_chunks_processed: None,
            })
        } else {
            SyncStatus::None
        };
        Ok(status)
    }
}

/// The default gas limit for eth_call and adjacent calls.
///
/// This is different from the default to regular 30M block gas limit
/// [ETHEREUM_BLOCK_GAS_LIMIT](reth_primitives::constants::ETHEREUM_BLOCK_GAS_LIMIT) to allow for
/// more complex calls.
pub const RPC_DEFAULT_GAS_CAP: GasCap = GasCap(50_000_000);

/// The wrapper type for gas limit
#[derive(Debug, Clone, Copy)]
pub struct GasCap(u64);

impl Default for GasCap {
    fn default() -> Self {
        RPC_DEFAULT_GAS_CAP
    }
}

impl From<u64> for GasCap {
    fn from(gas_cap: u64) -> Self {
        Self(gas_cap)
    }
}

impl From<GasCap> for u64 {
    fn from(gas_cap: GasCap) -> Self {
        gas_cap.0
    }
}

/// Container type `EthApi`
struct EthApiInner<Provider, Pool, Network> {
    /// The transaction pool.
    pool: Pool,
    /// The provider that can interact with the chain.
    provider: Provider,
    /// An interface to interact with the network
    network: Network,
    /// All configured Signers
    signers: Vec<Box<dyn EthSigner>>,
    /// The async cache frontend for eth related data
    eth_cache: EthStateCache,
    /// The async gas oracle frontend for gas price suggestions
    gas_oracle: GasPriceOracle<Provider>,
    /// Maximum gas limit for `eth_call` and call tracing RPC methods.
    gas_cap: u64,
    /// The block number at which the node started
    starting_block: U256,
    /// The type that can spawn tasks which would otherwise block.
    task_spawner: Box<dyn TaskSpawner>,
    /// Cached pending block if any
    pending_block: Mutex<Option<PendingBlock>>,
    /// A pool dedicated to blocking tasks.
    blocking_task_pool: BlockingTaskPool,
    /// Cache for block fees history
    fee_history_cache: FeeHistoryCache,
    /// An http client for communicating with sequencers.
    #[cfg(feature = "optimism")]
    http_client: reqwest::Client,
}

/// Settings for the [FeeHistoryCache](crate::eth::FeeHistoryCache).
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeeHistoryCacheConfig {
    /// Max number of blocks in cache.
    ///
    /// Default is 1024.
    pub max_blocks: u64,
    /// Percentile approximation resolution
    ///
    /// Default is 4 which means 0.25
    pub resolution: u64,
}

impl Default for FeeHistoryCacheConfig {
    fn default() -> Self {
        FeeHistoryCacheConfig { max_blocks: 1024, resolution: 4 }
    }
}

/// Wrapper struct for BTreeMap
#[derive(Debug, Clone)]
pub struct FeeHistoryCache {
    lower_bound: Arc<AtomicU64>,
    upper_bound: Arc<AtomicU64>,
    config: FeeHistoryCacheConfig,
    entries: Arc<tokio::sync::RwLock<BTreeMap<u64, FeeHistoryEntry>>>,
    eth_cache: EthStateCache,
}

impl FeeHistoryCache {
    /// Creates new FeeHistoryCache instance, initialize it with the mose recent data, set bounds
    pub fn new(eth_cache: EthStateCache, config: FeeHistoryCacheConfig) -> Self {
        let init_tree_map = BTreeMap::new();

        let entries = Arc::new(tokio::sync::RwLock::new(init_tree_map));

        let upper_bound = Arc::new(AtomicU64::new(0));
        let lower_bound = Arc::new(AtomicU64::new(0));

        FeeHistoryCache { config, entries, upper_bound, lower_bound, eth_cache }
    }

    /// Processing of the arriving blocks
    pub async fn on_new_blocks<'a, I>(&self, blocks: I)
    where
        I: Iterator<Item = &'a SealedBlock>,
    {
        let mut entries = self.entries.write().await;

        for block in blocks {
            let mut fee_history_entry = FeeHistoryEntry::new(&block);
            let percentiles = self.predefined_percentiles();

            if let Ok(Some((transactions, receipts))) =
                self.eth_cache.get_transactions_and_receipts(fee_history_entry.header_hash).await
            {
                fee_history_entry.rewards = calculate_reward_percentiles_for_block(
                    &percentiles,
                    &fee_history_entry,
                    transactions,
                    receipts,
                )
                .await
                .unwrap_or_default();

                entries.insert(block.number, fee_history_entry);
            } else {
                break;
            }
        }
        while entries.len() > self.config.max_blocks as usize {
            entries.pop_first();
        }
        if entries.len() == 0 {
            self.upper_bound.store(0, SeqCst);
            self.lower_bound.store(0, SeqCst);
            return;
        }
        let upper_bound = *entries.last_entry().expect("Contains at least one entry").key();
        let lower_bound = *entries.first_entry().expect("Contains at least one entry").key();
        self.upper_bound.store(upper_bound, SeqCst);
        self.lower_bound.store(lower_bound, SeqCst);
    }

    /// Get UpperBound value for FeeHistoryCache
    pub fn upper_bound(&self) -> u64 {
        self.upper_bound.load(SeqCst)
    }

    /// Get LowerBound value for FeeHistoryCache
    pub fn lower_bound(&self) -> u64 {
        self.lower_bound.load(SeqCst)
    }

    /// Collect fee history for given range.
    /// This function retrieves fee history entries from the cache for the specified range.
    /// If the requested range (star_block to end_block) is within the cache bounds,
    /// it returns the corresponding entries.
    /// Otherwise it returns None.
    pub async fn get_history(
        &self,
        start_block: u64,
        end_block: u64,
    ) -> RethResult<Vec<FeeHistoryEntry>> {
        let lower_bound = self.lower_bound();
        let upper_bound = self.upper_bound();
        if start_block >= lower_bound && end_block <= upper_bound {
            let entries = self.entries.read().await;
            let result = entries
                .range(start_block..=end_block + 1)
                .map(|(_, fee_entry)| fee_entry.clone())
                .collect();
            Ok(result)
        } else {
            Ok(Vec::new())
        }
    }

    /// Generates predefined set of percentiles
    pub fn predefined_percentiles(&self) -> Vec<f64> {
        (0..=100 * self.config.resolution)
            .map(|p| p as f64 / self.config.resolution as f64)
            .collect()
    }
}

/// Awaits for new chain events and directly inserts them into the cache so they're available
/// immediately before they need to be fetched from disk.
pub async fn fee_history_cache_new_blocks_task<St, Provider>(
    fee_history_cache: FeeHistoryCache,
    mut events: St,
    provider: Provider,
) where
    St: Stream<Item = CanonStateNotification> + Unpin + 'static,
    Provider: BlockReaderIdExt + ChainSpecProvider + 'static,
{
    // Init default state
    if fee_history_cache.upper_bound() == 0 {
        let last_block_number = provider.last_block_number().unwrap_or(0);

        let start_block = if last_block_number > fee_history_cache.config.max_blocks {
            last_block_number - fee_history_cache.config.max_blocks
        } else {
            0
        };

        let blocks = provider.block_range(start_block..=last_block_number).unwrap_or_default();
        let sealed = blocks.into_iter().map(|block| block.seal_slow()).collect::<Vec<_>>();

        fee_history_cache.on_new_blocks(sealed.iter()).await;
    }

    while let Some(event) = events.next().await {
        if let Some(committed) = event.committed() {
            // we're only interested in new committed blocks
            let (blocks, _) = committed.inner();

            let blocks = blocks.iter().map(|(_, v)| v.block.clone()).collect::<Vec<_>>();

            fee_history_cache.on_new_blocks(blocks.iter()).await;
        }
    }
}

/// Calculates reward percentiles for transactions in a block header.
/// Given a list of percentiles and a sealed block header, this function computes
/// the corresponding rewards for the transactions at each percentile.
///
/// The results are returned as a vector of U256 values.
async fn calculate_reward_percentiles_for_block(
    percentiles: &[f64],
    fee_entry: &FeeHistoryEntry,
    transactions: Vec<TransactionSigned>,
    receipts: Vec<Receipt>,
) -> Result<Vec<U256>, EthApiError> {
    let mut transactions = transactions
        .into_iter()
        .zip(receipts)
        .scan(0, |previous_gas, (tx, receipt)| {
            // Convert the cumulative gas used in the receipts
            // to the gas usage by the transaction
            //
            // While we will sum up the gas again later, it is worth
            // noting that the order of the transactions will be different,
            // so the sum will also be different for each receipt.
            let gas_used = receipt.cumulative_gas_used - *previous_gas;
            *previous_gas = receipt.cumulative_gas_used;

            Some(TxGasAndReward {
                gas_used,
                reward: tx
                    .effective_tip_per_gas(Some(fee_entry.base_fee_per_gas))
                    .unwrap_or_default(),
            })
        })
        .collect::<Vec<_>>();

    // Sort the transactions by their rewards in ascending order
    transactions.sort_by_key(|tx| tx.reward);

    // Find the transaction that corresponds to the given percentile
    //
    // We use a `tx_index` here that is shared across all percentiles, since we know
    // the percentiles are monotonically increasing.
    let mut tx_index = 0;
    let mut cumulative_gas_used = transactions.first().map(|tx| tx.gas_used).unwrap_or_default();
    let mut rewards_in_block = Vec::new();
    for percentile in percentiles {
        // Empty blocks should return in a zero row
        if transactions.is_empty() {
            rewards_in_block.push(U256::ZERO);
            continue;
        }

        let threshold = (fee_entry.gas_used as f64 * percentile / 100.) as u64;
        while cumulative_gas_used < threshold && tx_index < transactions.len() - 1 {
            tx_index += 1;
            cumulative_gas_used += transactions[tx_index].gas_used;
        }
        rewards_in_block.push(U256::from(transactions[tx_index].reward));
    }

    Ok(rewards_in_block)
}

#[derive(Debug, Clone)]
pub struct FeeHistoryEntry {
    base_fee_per_gas: u64,
    gas_used_ratio: f64,
    gas_used: u64,
    gas_limit: u64,
    header_hash: B256,
    rewards: Vec<U256>,
}

impl FeeHistoryEntry {
    fn new(block: &SealedBlock) -> Self {
        FeeHistoryEntry {
            base_fee_per_gas: block.base_fee_per_gas.unwrap_or_default(),
            gas_used_ratio: block.gas_used as f64 / block.gas_limit as f64,
            gas_used: block.gas_used,
            header_hash: block.hash,
            gas_limit: block.gas_limit,
            rewards: Vec::new(),
        }
    }
}
