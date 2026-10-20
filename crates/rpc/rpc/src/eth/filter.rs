//! `eth_` `Filter` RPC handler implementation

use alloy_consensus::BlockHeader;
use alloy_eips::BlockNumberOrTag;
use alloy_primitives::{Sealable, TxHash};
use alloy_rpc_types_eth::{
    error::EthRpcErrorCode, Filter, FilterBlockOption, FilterChanges, FilterId,
    PendingTransactionFilterKind,
};
use async_trait::async_trait;
use futures::{
    future::TryFutureExt,
    stream::{FuturesOrdered, StreamExt},
    Future,
};
use jsonrpsee::{core::RpcResult, server::IdProvider};
use reth_errors::ProviderError;
use reth_primitives_traits::{NodePrimitives, SealedHeader};
use reth_rpc_eth_api::{
    helpers::{EthBlocks, LoadReceipt},
    EngineEthFilter, EthApiTypes, EthFilterApiServer, FullEthApiTypes, QueryLimits, RpcConvert,
    RpcLog, RpcNodeCoreExt, RpcTransaction,
};
use reth_rpc_eth_types::{
    logs_utils::{self, append_matching_block_logs, ProviderOrBlock},
    EthApiError, EthFilterConfig, EthStateCache, EthSubscriptionIdProvider,
};
use reth_rpc_server_types::{result::rpc_error_with_code, ToRpcResult};
use reth_storage_api::{
    BlockHashReader, BlockIdReader, BlockNumReader, BlockReader, HeaderProvider, ProviderBlock,
    ProviderReceipt, ReceiptProvider,
};
use reth_tasks::Runtime;
use reth_transaction_pool::{NewSubpoolTransactionStream, PoolTransaction, TransactionPool};
use std::{
    collections::{HashMap, VecDeque},
    fmt,
    iter::StepBy,
    ops::RangeInclusive,
    pin::Pin,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    sync::{mpsc::Receiver, oneshot, Mutex},
    time::MissedTickBehavior,
};
use tracing::{debug, error, trace};

impl<Eth> EngineEthFilter<RpcLog<Eth::NetworkTypes>> for EthFilter<Eth>
where
    Eth: FullEthApiTypes + RpcNodeCoreExt<Provider: BlockIdReader> + LoadReceipt + EthBlocks,
{
    /// Returns logs matching given filter object, no query limits
    fn logs(
        &self,
        filter: Filter,
        limits: QueryLimits,
    ) -> impl Future<Output = RpcResult<Vec<RpcLog<Eth::NetworkTypes>>>> + Send {
        trace!(target: "rpc::eth", "Serving eth_getLogs");
        self.logs_for_filter(filter, limits).map_err(|e| e.into())
    }
}

/// Threshold for deciding between cached and range mode processing
const CACHED_MODE_BLOCK_THRESHOLD: u64 = 250;

/// Threshold for bloom filter matches that triggers reduced caching
const HIGH_BLOOM_MATCH_THRESHOLD: usize = 20;

/// Threshold for bloom filter matches that triggers moderately reduced caching
const MODERATE_BLOOM_MATCH_THRESHOLD: usize = 10;

/// Minimum block count to apply bloom filter match adjustments
const BLOOM_ADJUSTMENT_MIN_BLOCKS: u64 = 100;

/// Number of consecutive blocks a single task of a range scan reads
const RANGE_CHUNK_SIZE: u64 = 64;

/// Maximum number of chunks of a range scan that are read ahead of the returned logs
const MAX_PENDING_CHUNKS: usize = 8;

/// `Eth` filter RPC implementation.
///
/// This type handles `eth_` rpc requests related to filters (`eth_getLogs`).
pub struct EthFilter<Eth: EthApiTypes> {
    /// All nested fields bundled together
    inner: Arc<EthFilterInner<Eth>>,
}

impl<Eth> Clone for EthFilter<Eth>
where
    Eth: EthApiTypes,
{
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone() }
    }
}

impl<Eth> EthFilter<Eth>
where
    Eth: EthApiTypes + 'static,
{
    /// Creates a new, shareable instance.
    ///
    /// This uses the given pool to get notified about new transactions, the provider to interact
    /// with the blockchain, the cache to fetch cacheable data, like the logs.
    ///
    /// See also [`EthFilterConfig`].
    ///
    /// This also spawns a task that periodically clears stale filters.
    ///
    /// # Create a new instance with [`EthApi`](crate::EthApi)
    ///
    /// ```no_run
    /// use reth_evm_ethereum::EthEvmConfig;
    /// use reth_network_api::noop::NoopNetwork;
    /// use reth_provider::noop::NoopProvider;
    /// use reth_rpc::{EthApi, EthFilter};
    /// use reth_tasks::Runtime;
    /// use reth_transaction_pool::noop::NoopTransactionPool;
    /// let eth_api = EthApi::builder(
    ///     NoopProvider::default(),
    ///     NoopTransactionPool::default(),
    ///     NoopNetwork::default(),
    ///     EthEvmConfig::mainnet(),
    /// )
    /// .build();
    /// let filter = EthFilter::new(eth_api, Default::default(), Runtime::test());
    /// ```
    pub fn new(eth_api: Eth, config: EthFilterConfig, task_spawner: Runtime) -> Self {
        let EthFilterConfig { max_blocks_per_filter, max_logs_per_response, stale_filter_ttl } =
            config;
        let inner = EthFilterInner {
            eth_api,
            active_filters: ActiveFilters::new(),
            id_provider: Arc::new(EthSubscriptionIdProvider::default()),
            task_spawner,
            stale_filter_ttl,
            query_limits: QueryLimits { max_blocks_per_filter, max_logs_per_response },
        };

        let eth_filter = Self { inner: Arc::new(inner) };

        let this = eth_filter.clone();
        eth_filter.inner.task_spawner.spawn_critical_task(
            "eth-filters_stale-filters-clean",
            async move {
                this.watch_and_clear_stale_filters().await;
            },
        );

        eth_filter
    }

    /// Returns all currently active filters
    pub fn active_filters(&self) -> &ActiveFilters<RpcTransaction<Eth::NetworkTypes>> {
        &self.inner.active_filters
    }

    /// Endless future that [`Self::clear_stale_filters`] every `stale_filter_ttl` interval.
    /// Nonetheless, this endless future frees the thread at every await point.
    async fn watch_and_clear_stale_filters(&self) {
        let mut interval = tokio::time::interval_at(
            tokio::time::Instant::now() + self.inner.stale_filter_ttl,
            self.inner.stale_filter_ttl,
        );
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            self.clear_stale_filters(Instant::now()).await;
        }
    }

    /// Clears all filters that have not been polled for longer than the configured
    /// `stale_filter_ttl` at the given instant.
    pub async fn clear_stale_filters(&self, now: Instant) {
        trace!(target: "rpc::eth", "clear stale filters");
        let mut filters = self.active_filters().inner.lock().await;
        filters.retain(|id, filter| {
            let is_valid = (now - filter.last_poll_timestamp) < self.inner.stale_filter_ttl;

            if !is_valid {
                trace!(target: "rpc::eth", "evict filter with id: {:?}", id);
            }

            is_valid
        });
        filters.shrink_to_fit();
    }
}

impl<Eth> EthFilter<Eth>
where
    Eth: FullEthApiTypes<Provider: BlockReader + BlockIdReader> + LoadReceipt + EthBlocks,
{
    /// Access the underlying provider.
    fn provider(&self) -> &Eth::Provider {
        self.inner.eth_api.provider()
    }

    /// Access the underlying pool.
    fn pool(&self) -> &Eth::Pool {
        self.inner.eth_api.pool()
    }

    /// Returns all the filter changes for the given id, if any
    pub async fn filter_changes(
        &self,
        id: FilterId,
    ) -> Result<
        FilterChanges<RpcTransaction<Eth::NetworkTypes>, RpcLog<Eth::NetworkTypes>>,
        EthFilterError,
    > {
        let info = self.provider().chain_info()?;
        let best_number = info.best_number;

        // start_block is the block from which we should start fetching changes, the next block from
        // the last time changes were polled, in other words the best block at last poll + 1
        let (start_block, kind) = {
            let mut filters = self.inner.active_filters.inner.lock().await;
            let filter = filters.get_mut(&id).ok_or(EthFilterError::FilterNotFound(id))?;

            if filter.block > best_number {
                // no new blocks since the last poll
                return Ok(FilterChanges::Empty)
            }

            // update filter
            // we fetch all changes from [filter.block..best_block], so we advance the filter's
            // block to `best_block +1`, the next from which we should start fetching changes again
            let mut block = best_number + 1;
            std::mem::swap(&mut filter.block, &mut block);
            filter.last_poll_timestamp = Instant::now();

            (block, filter.kind.clone())
        };

        match kind {
            FilterKind::PendingTransaction(filter) => Ok(match filter.drain().await {
                FilterChanges::Empty => FilterChanges::Empty,
                FilterChanges::Hashes(hashes) => FilterChanges::Hashes(hashes),
                FilterChanges::Transactions(transactions) => {
                    FilterChanges::Transactions(transactions)
                }
                FilterChanges::Logs(_) => unreachable!("pending transaction filter returned logs"),
            }),
            FilterKind::Block => {
                // Note: we need to fetch the block hashes from inclusive range
                // [start_block..best_block]
                let end_block = best_number + 1;
                let block_hashes =
                    self.provider().canonical_hashes_range(start_block, end_block).map_err(
                        |_| EthApiError::HeaderRangeNotFound(start_block.into(), end_block.into()),
                    )?;
                Ok(FilterChanges::Hashes(block_hashes))
            }
            FilterKind::Log(filter) => {
                let (from_block_number, to_block_number) = match filter.block_option {
                    FilterBlockOption::Range { from_block, to_block } => {
                        let from = from_block
                            .map(|num| self.provider().convert_block_number(num))
                            .transpose()?
                            .flatten();
                        let to = to_block
                            .map(|num| self.provider().convert_block_number(num))
                            .transpose()?
                            .flatten();
                        logs_utils::get_filter_block_range(from, to, start_block, info)?
                    }
                    FilterBlockOption::AtBlockHash(block_hash) => {
                        // blockHash is equivalent to fromBlock = toBlock = the block number with
                        // hash blockHash
                        // get_logs_in_block_range is inclusive
                        let block_number = self
                            .provider()
                            .block_number(block_hash)?
                            .ok_or(ProviderError::HeaderNotFound(block_hash.into()))?;
                        (block_number, block_number)
                    }
                };
                let logs = self
                    .inner
                    .clone()
                    .get_logs_in_block_range(
                        *filter,
                        from_block_number,
                        to_block_number,
                        self.inner.query_limits,
                    )
                    .await?;
                Ok(FilterChanges::Logs(logs))
            }
        }
    }

    /// Returns an array of all logs matching filter with given id.
    ///
    /// Returns an error if no matching log filter exists.
    ///
    /// Handler for `eth_getFilterLogs`
    pub async fn filter_logs(
        &self,
        id: FilterId,
    ) -> Result<Vec<RpcLog<Eth::NetworkTypes>>, EthFilterError> {
        let filter = {
            let mut filters = self.inner.active_filters.inner.lock().await;
            let filter =
                filters.get_mut(&id).ok_or_else(|| EthFilterError::FilterNotFound(id.clone()))?;
            if let FilterKind::Log(ref inner_filter) = filter.kind {
                filter.last_poll_timestamp = Instant::now();
                *inner_filter.clone()
            } else {
                // Not a log filter
                return Err(EthFilterError::FilterNotFound(id))
            }
        };

        self.logs_for_filter(filter, self.inner.query_limits).await
    }

    /// Returns logs matching given filter object.
    async fn logs_for_filter(
        &self,
        filter: Filter,
        limits: QueryLimits,
    ) -> Result<Vec<RpcLog<Eth::NetworkTypes>>, EthFilterError> {
        self.inner.clone().logs_for_filter(filter, limits).await
    }
}

#[async_trait]
impl<Eth> EthFilterApiServer<RpcTransaction<Eth::NetworkTypes>, RpcLog<Eth::NetworkTypes>>
    for EthFilter<Eth>
where
    Eth: FullEthApiTypes + LoadReceipt + EthBlocks,
{
    /// Handler for `eth_newFilter`
    async fn new_filter(&self, filter: Filter) -> RpcResult<FilterId> {
        trace!(target: "rpc::eth", "Serving eth_newFilter");
        self.inner
            .install_filter(FilterKind::<RpcTransaction<Eth::NetworkTypes>>::Log(Box::new(filter)))
            .await
    }

    /// Handler for `eth_newBlockFilter`
    async fn new_block_filter(&self) -> RpcResult<FilterId> {
        trace!(target: "rpc::eth", "Serving eth_newBlockFilter");
        self.inner.install_filter(FilterKind::<RpcTransaction<Eth::NetworkTypes>>::Block).await
    }

    /// Handler for `eth_newPendingTransactionFilter`
    async fn new_pending_transaction_filter(
        &self,
        kind: Option<PendingTransactionFilterKind>,
    ) -> RpcResult<FilterId> {
        trace!(target: "rpc::eth", "Serving eth_newPendingTransactionFilter");

        let transaction_kind = match kind.unwrap_or_default() {
            PendingTransactionFilterKind::Hashes => {
                let receiver = self.pool().pending_transactions_listener();
                let pending_txs_receiver = PendingTransactionsReceiver::new(receiver);
                FilterKind::PendingTransaction(PendingTransactionKind::Hashes(pending_txs_receiver))
            }
            PendingTransactionFilterKind::Full => {
                let stream = self.pool().new_pending_pool_transactions_listener();
                let full_txs_receiver = FullTransactionsReceiver::new(
                    stream,
                    dyn_clone::clone(self.inner.eth_api.converter()),
                );
                FilterKind::PendingTransaction(PendingTransactionKind::FullTransaction(Arc::new(
                    full_txs_receiver,
                )))
            }
        };

        // Install the filter and propagate any errors
        self.inner.install_filter(transaction_kind).await
    }

    /// Handler for `eth_getFilterChanges`
    async fn filter_changes(
        &self,
        id: FilterId,
    ) -> RpcResult<FilterChanges<RpcTransaction<Eth::NetworkTypes>, RpcLog<Eth::NetworkTypes>>>
    {
        trace!(target: "rpc::eth", "Serving eth_getFilterChanges");
        Ok(Self::filter_changes(self, id).await?)
    }

    /// Returns an array of all logs matching filter with given id.
    ///
    /// Returns an error if no matching log filter exists.
    ///
    /// Handler for `eth_getFilterLogs`
    async fn filter_logs(&self, id: FilterId) -> RpcResult<Vec<RpcLog<Eth::NetworkTypes>>> {
        trace!(target: "rpc::eth", "Serving eth_getFilterLogs");
        Ok(Self::filter_logs(self, id).await?)
    }

    /// Handler for `eth_uninstallFilter`
    async fn uninstall_filter(&self, id: FilterId) -> RpcResult<bool> {
        trace!(target: "rpc::eth", "Serving eth_uninstallFilter");
        let mut filters = self.inner.active_filters.inner.lock().await;
        if filters.remove(&id).is_some() {
            trace!(target: "rpc::eth::filter", ?id, "uninstalled filter");
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Returns logs matching given filter object.
    ///
    /// Handler for `eth_getLogs`
    async fn logs(&self, filter: Filter) -> RpcResult<Vec<RpcLog<Eth::NetworkTypes>>> {
        trace!(target: "rpc::eth", "Serving eth_getLogs");
        Ok(self.logs_for_filter(filter, self.inner.query_limits).await?)
    }
}

impl<Eth> std::fmt::Debug for EthFilter<Eth>
where
    Eth: EthApiTypes,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EthFilter").finish_non_exhaustive()
    }
}

/// Container type `EthFilter`
#[derive(Debug)]
struct EthFilterInner<Eth: EthApiTypes> {
    /// Inner `eth` API implementation.
    eth_api: Eth,
    /// All currently installed filters.
    active_filters: ActiveFilters<RpcTransaction<Eth::NetworkTypes>>,
    /// Provides ids to identify filters
    id_provider: Arc<dyn IdProvider>,
    /// limits for logs queries
    query_limits: QueryLimits,
    /// The type that can spawn tasks.
    task_spawner: Runtime,
    /// Duration since the last filter poll, after which the filter is considered stale
    stale_filter_ttl: Duration,
}

impl<Eth> EthFilterInner<Eth>
where
    Eth: RpcNodeCoreExt<Provider: BlockIdReader, Pool: TransactionPool>
        + EthApiTypes<NetworkTypes: reth_rpc_eth_api::types::RpcTypes>
        + LoadReceipt
        + EthBlocks,
{
    /// Access the underlying provider.
    fn provider(&self) -> &Eth::Provider {
        self.eth_api.provider()
    }

    /// Access the underlying [`EthStateCache`].
    fn eth_cache(&self) -> &EthStateCache<Eth::Primitives> {
        self.eth_api.cache()
    }

    /// Returns logs matching given filter object.
    async fn logs_for_filter(
        self: Arc<Self>,
        filter: Filter,
        limits: QueryLimits,
    ) -> Result<Vec<RpcLog<Eth::NetworkTypes>>, EthFilterError> {
        match filter.block_option {
            FilterBlockOption::AtBlockHash(block_hash) => {
                // First try to get cached block and receipts, as it's likely they're already cached
                let Some((receipts, maybe_block)) =
                    self.eth_cache().get_receipts_and_maybe_block(block_hash).await?
                else {
                    // the block itself may still exist with its receipts pruned
                    return Err(match self.provider().block_number(block_hash)? {
                        Some(number) => {
                            let earliest_available = self.provider().earliest_block_number()?;
                            if number < earliest_available {
                                EthApiError::PrunedHistoryUnavailable {
                                    requested: number,
                                    earliest_available,
                                }
                                .into()
                            } else {
                                EthFilterError::ReceiptsUnavailable(number)
                            }
                        }
                        None => ProviderError::HeaderNotFound(block_hash.into()).into(),
                    })
                };

                let header = if let Some(block) = &maybe_block {
                    block.clone_sealed_header()
                } else {
                    let header = self
                        .provider()
                        .header_by_hash_or_number(block_hash.into())?
                        .ok_or_else(|| ProviderError::HeaderNotFound(block_hash.into()))?;
                    SealedHeader::new(header, block_hash)
                };

                // Check if the block has been pruned (EIP-4444)
                let earliest_block = self.provider().earliest_block_number()?;
                if header.number() < earliest_block {
                    return Err(EthApiError::PrunedHistoryUnavailable {
                        requested: header.number(),
                        earliest_available: earliest_block,
                    }
                    .into());
                }

                if !filter.matches_bloom(header.logs_bloom()) {
                    return Ok(Vec::new())
                }

                let mut all_logs = Vec::new();
                append_matching_block_logs(
                    &mut all_logs,
                    self.eth_api.converter(),
                    maybe_block
                        .map(ProviderOrBlock::Block)
                        .unwrap_or_else(|| ProviderOrBlock::Provider(self.provider())),
                    &filter,
                    &header,
                    &receipts,
                    false,
                )?;
                Ok(all_logs)
            }
            FilterBlockOption::Range { from_block, to_block } => {
                // Handle special case where from block is pending
                if from_block.is_some_and(|b| b.is_pending()) {
                    let to_block = to_block.unwrap_or(BlockNumberOrTag::Pending);
                    if !(to_block.is_pending() || to_block.is_number()) {
                        // always empty range
                        return Ok(Vec::new());
                    }
                    // Try to get pending block and receipts
                    if let Ok(Some(pending_block)) = self.eth_api.local_pending_block().await {
                        if let BlockNumberOrTag::Number(to_block) = to_block &&
                            to_block < pending_block.block.number()
                        {
                            // this block range is empty based on the user input
                            return Ok(Vec::new());
                        }

                        let info = self.provider().chain_info()?;
                        if pending_block.block.number() > info.best_number {
                            // only consider the pending block if it is ahead of the chain
                            let mut all_logs = Vec::new();
                            let header = pending_block.block.clone_sealed_header();
                            append_matching_block_logs(
                                &mut all_logs,
                                self.eth_api.converter(),
                                ProviderOrBlock::<Eth::Provider>::Block(pending_block.block),
                                &filter,
                                &header,
                                &pending_block.receipts,
                                false, // removed = false for pending blocks
                            )?;
                            return Ok(all_logs)
                        }
                    }
                }

                let info = self.provider().chain_info()?;
                let start_block = info.best_number;
                // Without a pending block to serve, a `pending` bound resolves to the head on both
                // ends instead of to whatever payload the engine currently holds
                let from = from_block
                    .filter(|num| !num.is_pending())
                    .map(|num| self.provider().convert_block_number(num))
                    .transpose()?
                    .flatten();
                let to = to_block
                    .filter(|num| !num.is_pending())
                    .map(|num| self.provider().convert_block_number(num))
                    .transpose()?
                    .flatten();

                // Return error if toBlock exceeds current head
                if let Some(t) = to &&
                    t > info.best_number
                {
                    return Err(EthFilterError::BlockRangeExceedsHead {
                        requested: t,
                        head: info.best_number,
                    });
                }

                let (from_block_number, to_block_number) =
                    logs_utils::get_filter_block_range(from, to, start_block, info)?;

                // Check if the requested range overlaps with pruned history (EIP-4444)
                let earliest_block = self.provider().earliest_block_number()?;
                if from_block_number < earliest_block {
                    return Err(EthApiError::PrunedHistoryUnavailable {
                        requested: from_block_number,
                        earliest_available: earliest_block,
                    }
                    .into());
                }

                self.get_logs_in_block_range(filter, from_block_number, to_block_number, limits)
                    .await
            }
        }
    }

    /// Installs a new filter and returns the new identifier.
    async fn install_filter(
        &self,
        kind: FilterKind<RpcTransaction<Eth::NetworkTypes>>,
    ) -> RpcResult<FilterId> {
        let last_poll_block_number = self.provider().best_block_number().to_rpc_result()?;
        let subscription_id = self.id_provider.next_id();

        let id = match subscription_id {
            jsonrpsee_types::SubscriptionId::Num(n) => FilterId::Num(n),
            jsonrpsee_types::SubscriptionId::Str(s) => FilterId::Str(s.into_owned()),
        };
        let mut filters = self.active_filters.inner.lock().await;
        filters.insert(
            id.clone(),
            ActiveFilter {
                block: last_poll_block_number,
                last_poll_timestamp: Instant::now(),
                kind,
            },
        );
        Ok(id)
    }

    /// Returns all logs in the given _inclusive_ range that match the filter
    ///
    /// Returns an error if:
    ///  - underlying database error
    ///  - amount of matches exceeds configured limit
    async fn get_logs_in_block_range(
        self: Arc<Self>,
        filter: Filter,
        from_block: u64,
        to_block: u64,
        limits: QueryLimits,
    ) -> Result<Vec<RpcLog<Eth::NetworkTypes>>, EthFilterError> {
        trace!(target: "rpc::eth::filter", from=from_block, to=to_block, ?filter, "finding logs in range");

        // perform boundary checks first
        if to_block < from_block {
            return Err(EthFilterError::InvalidBlockRangeParams)
        }

        if let Some(max_blocks_per_filter) =
            limits.max_blocks_per_filter.filter(|limit| to_block - from_block > *limit)
        {
            return Err(EthFilterError::QueryExceedsMaxBlocks(max_blocks_per_filter))
        }

        // The scan occupies a blocking thread until it completes, so it shares the budget for
        // blocking IO requests with `eth_call` and friends instead of pinning an unbounded number
        // of pool threads.
        let permit = self
            .eth_api
            .acquire_owned_blocking_io()
            .await
            .map_err(|_| EthFilterError::InternalError)?;

        let (mut tx, rx) = oneshot::channel();
        let this = self.clone();
        self.task_spawner.spawn_blocking_task(async move {
            let _permit = permit;
            let fut = this.get_logs_in_block_range_inner(&filter, from_block, to_block, limits);
            tokio::pin!(fut);
            let res = tokio::select! {
                // Range scans perform blocking reads before their first yield.
                biased;
                _ = tx.closed() => None,
                res = &mut fut => Some(res),
            };
            if let Some(res) = res {
                let _ = tx.send(res);
            }
        });

        rx.await.map_err(|_| EthFilterError::InternalError)?
    }

    /// Returns all logs in the given _inclusive_ range that match the filter
    ///
    /// Note: This function uses a mix of blocking db operations for fetching indices and header
    /// ranges and utilizes the rpc cache for optimistically fetching receipts and blocks.
    /// This function is considered blocking and should thus be spawned on a blocking task.
    ///
    /// Returns an error if:
    ///  - underlying database error
    async fn get_logs_in_block_range_inner(
        self: Arc<Self>,
        filter: &Filter,
        from_block: u64,
        to_block: u64,
        limits: QueryLimits,
    ) -> Result<Vec<RpcLog<Eth::NetworkTypes>>, EthFilterError> {
        let mut all_logs = Vec::new();

        // get current chain tip to determine processing mode
        let chain_tip = self.provider().best_block_number()?;

        // initialize the appropriate range mode based on the range
        let mut range_mode = RangeMode::new(self.clone(), filter, from_block, to_block, chain_tip)?;

        // iterate through the range mode to get the logs of each block
        while let Some((block_number, logs)) = range_mode.next(filter).await? {
            all_logs.extend(logs);

            // size check but only if range is multiple blocks, so we always return all
            // logs of a single block
            let is_multi_block_range = from_block != to_block;
            if let Some(max_logs_per_response) = limits.max_logs_per_response &&
                is_multi_block_range &&
                all_logs.len() > max_logs_per_response
            {
                let retry_to_block =
                    if block_number == from_block { from_block } else { block_number - 1 };

                debug!(
                    target: "rpc::eth::filter",
                    logs_found = all_logs.len(),
                    max_logs_per_response,
                    from_block,
                    to_block = retry_to_block,
                    "Query exceeded max logs per response limit"
                );
                return Err(EthFilterError::QueryExceedsMaxResults {
                    max_logs: max_logs_per_response,
                    from_block,
                    to_block: retry_to_block,
                });
            }
        }

        Ok(all_logs)
    }

    /// Returns the headers of the blocks in the given _inclusive_ range whose bloom matches the
    /// filter.
    ///
    /// This performs blocking reads.
    fn bloom_matching_headers(
        &self,
        filter: &Filter,
        from_block: u64,
        to_block: u64,
    ) -> Result<Vec<SealedHeader<<Eth::Provider as HeaderProvider>::Header>>, EthFilterError> {
        let mut matching_headers = Vec::new();
        let headers = self.provider().headers_range(from_block..=to_block)?;

        let mut headers_iter = headers.into_iter().peekable();

        while let Some(header) = headers_iter.next() {
            if !filter.matches_bloom(header.logs_bloom()) {
                continue
            }

            let current_number = header.number();

            let block_hash = match headers_iter.peek() {
                Some(next_header) if next_header.number() == current_number + 1 => {
                    // Headers are consecutive, use the more efficient parent_hash
                    next_header.parent_hash()
                }
                _ => {
                    // Headers not consecutive or last header, calculate hash
                    header.hash_slow()
                }
            };

            matching_headers.push(SealedHeader::new(header, block_hash));
        }

        Ok(matching_headers)
    }
}

/// All active filters
#[derive(Debug, Clone, Default)]
pub struct ActiveFilters<T> {
    inner: Arc<Mutex<HashMap<FilterId, ActiveFilter<T>>>>,
}

impl<T> ActiveFilters<T> {
    /// Returns an empty instance.
    pub fn new() -> Self {
        Self { inner: Arc::new(Mutex::new(HashMap::default())) }
    }

    /// Returns `true` if a filter with the given id exists.
    pub async fn contains(&self, id: &FilterId) -> bool {
        self.inner.lock().await.contains_key(id)
    }

    /// Returns the number of currently active filters.
    pub async fn len(&self) -> usize {
        self.inner.lock().await.len()
    }

    /// Returns `true` if there are no active filters.
    pub async fn is_empty(&self) -> bool {
        self.inner.lock().await.is_empty()
    }

    /// Returns all active filter ids.
    pub async fn ids(&self) -> Vec<FilterId> {
        self.inner.lock().await.keys().cloned().collect()
    }
}

/// An installed filter
#[derive(Debug)]
struct ActiveFilter<T> {
    /// At which block the filter was polled last.
    block: u64,
    /// Last time this filter was polled.
    last_poll_timestamp: Instant,
    /// What kind of filter it is.
    kind: FilterKind<T>,
}

/// A receiver for pending transactions that returns all new transactions since the last poll.
#[derive(Debug, Clone)]
struct PendingTransactionsReceiver {
    txs_receiver: Arc<Mutex<Receiver<TxHash>>>,
}

impl PendingTransactionsReceiver {
    fn new(receiver: Receiver<TxHash>) -> Self {
        Self { txs_receiver: Arc::new(Mutex::new(receiver)) }
    }

    /// Returns all new pending transactions received since the last poll.
    async fn drain<T>(&self) -> FilterChanges<T> {
        let mut pending_txs = Vec::new();
        let mut prepared_stream = self.txs_receiver.lock().await;

        while let Ok(tx_hash) = prepared_stream.try_recv() {
            pending_txs.push(tx_hash);
        }

        // Convert the vector of hashes into FilterChanges::Hashes
        FilterChanges::Hashes(pending_txs)
    }
}

/// A structure to manage and provide access to a stream of full transaction details.
#[derive(Debug, Clone)]
struct FullTransactionsReceiver<T: PoolTransaction, TxCompat> {
    txs_stream: Arc<Mutex<NewSubpoolTransactionStream<T>>>,
    converter: TxCompat,
}

impl<T, TxCompat> FullTransactionsReceiver<T, TxCompat>
where
    T: PoolTransaction + 'static,
    TxCompat: RpcConvert<Primitives: NodePrimitives<SignedTx = T::Consensus>>,
{
    /// Creates a new `FullTransactionsReceiver` encapsulating the provided transaction stream.
    fn new(stream: NewSubpoolTransactionStream<T>, converter: TxCompat) -> Self {
        Self { txs_stream: Arc::new(Mutex::new(stream)), converter }
    }

    /// Returns all new pending transactions received since the last poll.
    async fn drain(&self) -> FilterChanges<RpcTransaction<TxCompat::Network>> {
        let mut pending_txs = Vec::new();
        let mut prepared_stream = self.txs_stream.lock().await;

        while let Ok(tx) = prepared_stream.try_recv() {
            match self.converter.fill_pending(tx.transaction.to_consensus()) {
                Ok(tx) => pending_txs.push(tx),
                Err(err) => {
                    error!(target: "rpc",
                        %err,
                        "Failed to fill txn with block context"
                    );
                }
            }
        }
        FilterChanges::Transactions(pending_txs)
    }
}

/// Helper trait for [`FullTransactionsReceiver`] to erase the `Transaction` type.
#[async_trait]
trait FullTransactionsFilter<T>: fmt::Debug + Send + Sync + Unpin + 'static {
    async fn drain(&self) -> FilterChanges<T>;
}

#[async_trait]
impl<T, TxCompat> FullTransactionsFilter<RpcTransaction<TxCompat::Network>>
    for FullTransactionsReceiver<T, TxCompat>
where
    T: PoolTransaction + 'static,
    TxCompat: RpcConvert<Primitives: NodePrimitives<SignedTx = T::Consensus>>,
{
    async fn drain(&self) -> FilterChanges<RpcTransaction<TxCompat::Network>> {
        Self::drain(self).await
    }
}

/// Represents the kind of pending transaction data that can be retrieved.
///
/// This enum differentiates between two kinds of pending transaction data:
/// - Just the transaction hashes.
/// - Full transaction details.
#[derive(Debug, Clone)]
enum PendingTransactionKind<T> {
    Hashes(PendingTransactionsReceiver),
    FullTransaction(Arc<dyn FullTransactionsFilter<T>>),
}

impl<T: 'static> PendingTransactionKind<T> {
    async fn drain(&self) -> FilterChanges<T> {
        match self {
            Self::Hashes(receiver) => receiver.drain().await,
            Self::FullTransaction(receiver) => receiver.drain().await,
        }
    }
}

#[derive(Clone, Debug)]
enum FilterKind<T> {
    Log(Box<Filter>),
    Block,
    PendingTransaction(PendingTransactionKind<T>),
}

/// An iterator that yields _inclusive_ block ranges of a given step size
#[derive(Debug)]
struct BlockRangeInclusiveIter {
    iter: StepBy<RangeInclusive<u64>>,
    step: u64,
    end: u64,
}

impl BlockRangeInclusiveIter {
    fn new(range: RangeInclusive<u64>, step: u64) -> Self {
        Self { end: *range.end(), iter: range.step_by(step as usize + 1), step }
    }
}

impl Iterator for BlockRangeInclusiveIter {
    type Item = (u64, u64);

    fn next(&mut self) -> Option<Self::Item> {
        let start = self.iter.next()?;
        let end = (start + self.step).min(self.end);
        if start > end {
            return None
        }
        Some((start, end))
    }
}

/// Errors that can occur in the handler implementation
#[derive(Debug, thiserror::Error)]
pub enum EthFilterError {
    /// Filter not found.
    #[error("filter not found")]
    FilterNotFound(FilterId),
    /// Invalid block range.
    #[error("invalid block range params")]
    InvalidBlockRangeParams,
    /// Block range extends beyond current head.
    #[error("block range extends beyond current head block: requested {requested}, head {head}")]
    BlockRangeExceedsHead {
        /// The requested `toBlock` number
        requested: u64,
        /// The current head block number
        head: u64,
    },
    /// Query scope is too broad.
    #[error("query exceeds max block range {0}")]
    QueryExceedsMaxBlocks(u64),
    /// Receipts of a block the filter matched are gone, most likely pruned.
    #[error("pruned history unavailable")]
    ReceiptsUnavailable(u64),
    /// Query result is too large.
    #[error("query exceeds max results {max_logs}, retry with the range {from_block}-{to_block}")]
    QueryExceedsMaxResults {
        /// Maximum number of logs allowed per response
        max_logs: usize,
        /// Start block of the suggested retry range
        from_block: u64,
        /// End block of the suggested retry range (last successfully processed block)
        to_block: u64,
    },
    /// Error serving request in `eth_` namespace.
    #[error(transparent)]
    EthAPIError(#[from] EthApiError),
    /// Error thrown when a spawned task failed to deliver a response.
    #[error("internal filter error")]
    InternalError,
}

impl From<EthFilterError> for jsonrpsee::types::error::ErrorObject<'static> {
    fn from(err: EthFilterError) -> Self {
        match err {
            // geth and Nethermind answer -32000 for unknown filter ids
            EthFilterError::FilterNotFound(_) => rpc_error_with_code(
                jsonrpsee::types::error::CALL_EXECUTION_FAILED_CODE,
                "filter not found",
            ),
            err @ EthFilterError::InternalError => {
                rpc_error_with_code(jsonrpsee::types::error::INTERNAL_ERROR_CODE, err.to_string())
            }
            EthFilterError::EthAPIError(err) => err.into(),
            err @ EthFilterError::ReceiptsUnavailable(_) => {
                rpc_error_with_code(EthRpcErrorCode::PrunedHistory.code(), err.to_string())
            }
            err @ (EthFilterError::InvalidBlockRangeParams |
            EthFilterError::QueryExceedsMaxBlocks(_) |
            EthFilterError::QueryExceedsMaxResults { .. } |
            EthFilterError::BlockRangeExceedsHead { .. }) => {
                rpc_error_with_code(jsonrpsee::types::error::INVALID_PARAMS_CODE, err.to_string())
            }
        }
    }
}

impl From<ProviderError> for EthFilterError {
    fn from(err: ProviderError) -> Self {
        Self::EthAPIError(err.into())
    }
}

impl From<logs_utils::FilterBlockRangeError> for EthFilterError {
    fn from(err: logs_utils::FilterBlockRangeError) -> Self {
        match err {
            logs_utils::FilterBlockRangeError::InvalidBlockRange => Self::InvalidBlockRangeParams,
            logs_utils::FilterBlockRangeError::BlockRangeExceedsHead { requested, head } => {
                Self::BlockRangeExceedsHead { requested, head }
            }
        }
    }
}

/// Helper type for the common pattern of returning receipts, block and the original header that is
/// a match for the filter.
struct ReceiptBlockResult<P>
where
    P: BlockReader,
{
    /// We always need the entire receipts for the matching block.
    receipts: Arc<Vec<ProviderReceipt<P>>>,
    /// Block can be optional and we can fetch it lazily when needed.
    recovered_block: Option<Arc<reth_primitives_traits::RecoveredBlock<ProviderBlock<P>>>>,
    /// The header of the block.
    header: SealedHeader<<P as HeaderProvider>::Header>,
}

/// Represents different modes for processing block ranges when filtering logs
enum RangeMode<
    Eth: RpcNodeCoreExt<Provider: BlockIdReader, Pool: TransactionPool> + LoadReceipt + EthBlocks,
> {
    /// Use cache-based processing for recent blocks
    Cached(CachedMode<Eth>),
    /// Use range-based processing for older blocks
    Range(RangeBlockMode<Eth>),
}

impl<
        Eth: RpcNodeCoreExt<Provider: BlockIdReader, Pool: TransactionPool> + LoadReceipt + EthBlocks,
    > RangeMode<Eth>
{
    /// Creates a new `RangeMode` for the given _inclusive_ range.
    ///
    /// This performs blocking reads.
    fn new(
        filter_inner: Arc<EthFilterInner<Eth>>,
        filter: &Filter,
        from_block: u64,
        to_block: u64,
        chain_tip: u64,
    ) -> Result<Self, EthFilterError> {
        let block_count = to_block - from_block + 1;
        let distance_from_tip = chain_tip.saturating_sub(to_block);

        // Only a small range close to the tip can use cached mode, which also depends on how many
        // headers match the bloom
        if block_count <= CACHED_MODE_BLOCK_THRESHOLD &&
            distance_from_tip <= CACHED_MODE_BLOCK_THRESHOLD
        {
            let sealed_headers =
                filter_inner.bloom_matching_headers(filter, from_block, to_block)?;

            // Determine if we should use cached mode based on range characteristics
            if Self::should_use_cached_mode(&sealed_headers, block_count, distance_from_tip) {
                return Ok(Self::Cached(CachedMode {
                    filter_inner,
                    headers_iter: sealed_headers.into_iter(),
                }))
            }
        }

        Ok(Self::Range(RangeBlockMode {
            filter_inner,
            chunks: BlockRangeInclusiveIter::new(from_block..=to_block, RANGE_CHUNK_SIZE - 1),
            next: VecDeque::new(),
            pending_tasks: FuturesOrdered::new(),
        }))
    }

    /// Determines whether to use cached mode based on bloom filter matches and range size
    const fn should_use_cached_mode(
        headers: &[SealedHeader<<Eth::Provider as HeaderProvider>::Header>],
        block_count: u64,
        distance_from_tip: u64,
    ) -> bool {
        // Headers are already filtered by bloom, so count equals length
        let bloom_matches = headers.len();

        // Calculate adjusted threshold based on bloom matches
        let adjusted_threshold = Self::calculate_adjusted_threshold(block_count, bloom_matches);

        block_count <= adjusted_threshold && distance_from_tip <= adjusted_threshold
    }

    /// Calculates the adjusted cache threshold based on bloom filter matches
    const fn calculate_adjusted_threshold(block_count: u64, bloom_matches: usize) -> u64 {
        // Only apply adjustments for larger ranges
        if block_count <= BLOOM_ADJUSTMENT_MIN_BLOCKS {
            return CACHED_MODE_BLOCK_THRESHOLD;
        }

        match bloom_matches {
            n if n > HIGH_BLOOM_MATCH_THRESHOLD => CACHED_MODE_BLOCK_THRESHOLD / 2,
            n if n > MODERATE_BLOOM_MATCH_THRESHOLD => (CACHED_MODE_BLOCK_THRESHOLD * 3) / 4,
            _ => CACHED_MODE_BLOCK_THRESHOLD,
        }
    }

    /// Gets the logs matching the filter of the next block that has any.
    async fn next(
        &mut self,
        filter: &Filter,
    ) -> Result<Option<BlockLogs<Eth::NetworkTypes>>, EthFilterError> {
        match self {
            Self::Cached(cached) => {
                while let Some(ReceiptBlockResult { receipts, recovered_block, header }) =
                    cached.next().await?
                {
                    let mut logs = Vec::new();
                    append_matching_block_logs(
                        &mut logs,
                        cached.filter_inner.eth_api.converter(),
                        recovered_block.map(ProviderOrBlock::Block).unwrap_or_else(|| {
                            ProviderOrBlock::Provider(cached.filter_inner.provider())
                        }),
                        filter,
                        &header,
                        &receipts,
                        false,
                    )?;
                    if !logs.is_empty() {
                        return Ok(Some((header.number(), logs)))
                    }
                }
                Ok(None)
            }
            Self::Range(range) => range.next(filter).await,
        }
    }
}

/// Mode for processing blocks using cache optimization for recent blocks
struct CachedMode<
    Eth: RpcNodeCoreExt<Provider: BlockIdReader, Pool: TransactionPool> + LoadReceipt + EthBlocks,
> {
    filter_inner: Arc<EthFilterInner<Eth>>,
    headers_iter: std::vec::IntoIter<SealedHeader<<Eth::Provider as HeaderProvider>::Header>>,
}

impl<
        Eth: RpcNodeCoreExt<Provider: BlockIdReader, Pool: TransactionPool> + LoadReceipt + EthBlocks,
    > CachedMode<Eth>
{
    async fn next(&mut self) -> Result<Option<ReceiptBlockResult<Eth::Provider>>, EthFilterError> {
        let Some(header) = self.headers_iter.next() else { return Ok(None) };

        // Use get_receipts_and_maybe_block which has automatic fallback to provider
        let Some((receipts, maybe_block)) =
            self.filter_inner.eth_cache().get_receipts_and_maybe_block(header.hash()).await?
        else {
            return Err(EthFilterError::ReceiptsUnavailable(header.number()))
        };

        Ok(Some(ReceiptBlockResult { receipts, recovered_block: maybe_block, header }))
    }
}

/// The logs of a block that match a filter, with the number of the block.
type BlockLogs<N> = (u64, Vec<RpcLog<N>>);

/// Type alias for the chunk scanning task futures used in `RangeBlockMode`
type ChunkLogsFuture<N> =
    Pin<Box<dyn Future<Output = Vec<Result<BlockLogs<N>, EthFilterError>>> + Send>>;

/// Mode for processing blocks using range queries for older blocks
///
/// The range is split into chunks of consecutive blocks that are scanned on parallel tasks, ahead
/// of the logs returned: the log limit can end the query early, memory is bounded by the pending
/// chunks, and a cancelled query stops at the next chunk.
struct RangeBlockMode<
    Eth: RpcNodeCoreExt<Provider: BlockIdReader, Pool: TransactionPool> + LoadReceipt + EthBlocks,
> {
    filter_inner: Arc<EthFilterInner<Eth>>,
    /// The chunks that are not scanned yet
    chunks: BlockRangeInclusiveIter,
    /// The results of the last scanned chunk that were not returned yet
    next: VecDeque<Result<BlockLogs<Eth::NetworkTypes>, EthFilterError>>,
    /// The tasks scanning the next chunks, in block order
    pending_tasks: FuturesOrdered<ChunkLogsFuture<Eth::NetworkTypes>>,
}

impl<
        Eth: RpcNodeCoreExt<Provider: BlockIdReader, Pool: TransactionPool> + LoadReceipt + EthBlocks,
    > RangeBlockMode<Eth>
{
    async fn next(
        &mut self,
        filter: &Filter,
    ) -> Result<Option<BlockLogs<Eth::NetworkTypes>>, EthFilterError> {
        loop {
            // Keep the next chunks scanning while the logs of this one are consumed
            while self.pending_tasks.len() < MAX_PENDING_CHUNKS &&
                let Some((from, to)) = self.chunks.next()
            {
                self.spawn_chunk_task(filter.clone(), from, to);
            }

            if let Some(result) = self.next.pop_front() {
                return result.map(Some)
            }

            // a chunk may be scanned on this task, this gives the cancellation check a chance to
            // run
            tokio::task::yield_now().await;

            let Some(chunk_results) = self.pending_tasks.next().await else { return Ok(None) };
            self.next.extend(chunk_results);
        }
    }

    /// Spawns a task that scans the given _inclusive_ chunk of blocks.
    fn spawn_chunk_task(&mut self, filter: Filter, from_block: u64, to_block: u64) {
        let filter_inner = self.filter_inner.clone();
        let scan = move || Self::chunk_logs(&filter_inner, &filter, from_block, to_block);

        // A parallel task occupies an additional blocking thread, so it needs its own share
        // of the blocking IO budget. Without one the chunk is scanned on this task instead.
        let chunk_task: ChunkLogsFuture<Eth::NetworkTypes> = match self
            .filter_inner
            .eth_api
            .blocking_io_task_guard()
            .clone()
            .try_acquire_owned()
        {
            Ok(permit) => Box::pin(async move {
                let chunk_task = tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    scan()
                });

                // Await the blocking task and handle the result
                match chunk_task.await {
                    Ok(chunk_results) => chunk_results,
                    Err(join_err) => {
                        trace!(target: "rpc::eth::filter", error = ?join_err, "Task join error");
                        vec![Err(EthFilterError::InternalError)]
                    }
                }
            }),
            Err(_) => Box::pin(async move { scan() }),
        };

        self.pending_tasks.push_back(chunk_task);
    }

    /// Returns the logs matching the filter of each block in the given _inclusive_ range that
    /// has any, in block order.
    ///
    /// The scan stops at the first error, which is the last entry, so that the logs before it
    /// still count towards the log limit. This performs blocking reads.
    fn chunk_logs(
        filter_inner: &EthFilterInner<Eth>,
        filter: &Filter,
        from_block: u64,
        to_block: u64,
    ) -> Vec<Result<BlockLogs<Eth::NetworkTypes>, EthFilterError>> {
        let mut chunk_results = Vec::new();
        let scan: Result<(), EthFilterError> = (|| {
            for header in filter_inner.bloom_matching_headers(filter, from_block, to_block)? {
                // Fetch directly from provider - RangeMode is used for older blocks
                // unlikely to be cached
                let Some(receipts) =
                    filter_inner.provider().receipts_by_block(header.hash().into())?
                else {
                    return Err(EthFilterError::ReceiptsUnavailable(header.number()))
                };

                let mut logs = Vec::new();
                append_matching_block_logs(
                    &mut logs,
                    filter_inner.eth_api.converter(),
                    ProviderOrBlock::Provider(filter_inner.provider()),
                    filter,
                    &header,
                    &receipts,
                    false,
                )?;
                if !logs.is_empty() {
                    chunk_results.push(Ok((header.number(), logs)));
                }
            }
            Ok(())
        })();

        if let Err(err) = scan {
            chunk_results.push(Err(err));
        }
        chunk_results
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{eth::EthApi, EthApiBuilder};
    use alloy_network::Ethereum;
    use alloy_primitives::{Bloom, FixedBytes};
    use rand::Rng;
    use reth_chainspec::{ChainSpec, ChainSpecProvider};
    use reth_ethereum_primitives::TxType;
    use reth_evm_ethereum::EthEvmConfig;
    use reth_network_api::noop::NoopNetwork;
    use reth_provider::test_utils::MockEthProvider;
    use reth_rpc_convert::RpcConverter;
    use reth_rpc_eth_api::node::RpcNodeCoreAdapter;
    use reth_rpc_eth_types::receipt::EthReceiptConverter;
    use reth_tasks::Runtime;
    use reth_testing_utils::generators;
    use reth_transaction_pool::test_utils::{testing_pool, TestPool};

    #[test]
    fn receipts_unavailable_error_matches_geth() {
        let err: jsonrpsee::types::error::ErrorObject<'static> =
            EthFilterError::ReceiptsUnavailable(100).into();
        assert_eq!(err.code(), 4444);
        assert_eq!(err.message(), "pruned history unavailable");
    }

    #[test]
    fn test_block_range_iter() {
        let mut rng = generators::rng();

        let start = rng.random::<u32>() as u64;
        let end = start.saturating_add(rng.random::<u32>() as u64);
        let step = rng.random::<u16>() as u64;
        let range = start..=end;
        let mut iter = BlockRangeInclusiveIter::new(range.clone(), step);
        let (from, mut end) = iter.next().unwrap();
        assert_eq!(from, start);
        assert_eq!(end, (from + step).min(*range.end()));

        for (next_from, next_end) in iter {
            // ensure range starts with previous end + 1
            assert_eq!(next_from, end + 1);
            end = next_end;
        }

        assert_eq!(end, *range.end());
    }

    // Helper function to create a test EthApi instance
    #[expect(clippy::type_complexity)]
    fn build_test_eth_api(
        provider: MockEthProvider,
    ) -> EthApi<
        RpcNodeCoreAdapter<MockEthProvider, TestPool, NoopNetwork, EthEvmConfig>,
        RpcConverter<Ethereum, EthEvmConfig, EthReceiptConverter<ChainSpec>>,
    > {
        EthApiBuilder::new(
            provider.clone(),
            testing_pool(),
            NoopNetwork::default(),
            EthEvmConfig::new(provider.chain_spec()),
        )
        .build()
    }

    #[tokio::test]
    async fn test_logs_for_filter_from_block_beyond_head() {
        let provider = MockEthProvider::default();
        provider.add_header(FixedBytes::random(), alloy_consensus::Header::default());
        let eth_api = build_test_eth_api(provider);

        let eth_filter =
            super::EthFilter::new(eth_api, EthFilterConfig::default(), Runtime::test());

        let filter = Filter::new().from_block(100u64).to_block(BlockNumberOrTag::Latest);
        let result = eth_filter.inner.clone().logs_for_filter(filter, QueryLimits::default()).await;
        assert!(matches!(result, Err(EthFilterError::InvalidBlockRangeParams)), "{result:?}");
    }

    #[tokio::test]
    async fn test_cached_mode_with_mock_receipts() {
        // create test data
        let test_hash = FixedBytes::from([42u8; 32]);
        let test_block_number = 100u64;
        let test_header = SealedHeader::new(
            alloy_consensus::Header {
                number: test_block_number,
                gas_used: 50_000,
                ..Default::default()
            },
            test_hash,
        );

        // add a mock receipt to the provider with a mock log
        let mock_log = alloy_primitives::Log {
            address: alloy_primitives::Address::ZERO,
            data: alloy_primitives::LogData::new_unchecked(vec![], alloy_primitives::Bytes::new()),
        };

        let mock_receipt = reth_ethereum_primitives::Receipt {
            tx_type: TxType::Legacy,
            cumulative_gas_used: 21_000,
            logs: vec![mock_log],
            success: true,
        };

        let provider = MockEthProvider::default();
        provider.add_header(test_hash, test_header.header().clone());
        provider.add_receipts(test_block_number, vec![mock_receipt.clone()]);

        let eth_api = build_test_eth_api(provider);
        let eth_filter =
            super::EthFilter::new(eth_api, EthFilterConfig::default(), Runtime::test());
        let filter_inner = eth_filter.inner;

        let headers = vec![test_header.clone()];

        let mut cached_mode = CachedMode { filter_inner, headers_iter: headers.into_iter() };

        // should find the receipt from provider fallback (cache will be empty)
        let result = cached_mode.next().await.expect("next should succeed");
        let receipt_block_result = result.expect("should have receipt result");
        assert_eq!(receipt_block_result.header.hash(), test_hash);
        assert_eq!(receipt_block_result.header.number, test_block_number);
        assert_eq!(receipt_block_result.receipts.len(), 1);
        assert_eq!(receipt_block_result.receipts[0].tx_type, mock_receipt.tx_type);
        assert_eq!(
            receipt_block_result.receipts[0].cumulative_gas_used,
            mock_receipt.cumulative_gas_used
        );
        assert_eq!(receipt_block_result.receipts[0].success, mock_receipt.success);

        // iterator should be exhausted
        let result2 = cached_mode.next().await;
        assert!(result2.is_ok());
        assert!(result2.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_cached_mode_empty_headers() {
        let provider = MockEthProvider::default();
        let eth_api = build_test_eth_api(provider);

        let eth_filter =
            super::EthFilter::new(eth_api, EthFilterConfig::default(), Runtime::test());
        let filter_inner = eth_filter.inner;

        let headers: Vec<SealedHeader<alloy_consensus::Header>> = vec![];

        let mut cached_mode = CachedMode { filter_inner, headers_iter: headers.into_iter() };

        // should immediately return None for empty headers
        let result = cached_mode.next().await.expect("next should succeed");
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_log_limit_retry_range_excludes_overflow_block() {
        let provider = MockEthProvider::default();

        use alloy_consensus::TxLegacy;
        use reth_db_api::models::StoredBlockBodyIndices;
        use reth_ethereum_primitives::{TransactionSigned, TxType};

        let tx_inner = TxLegacy {
            chain_id: Some(1),
            nonce: 0,
            gas_price: 21_000,
            gas_limit: 21_000,
            to: alloy_primitives::TxKind::Call(alloy_primitives::Address::ZERO),
            value: alloy_primitives::U256::ZERO,
            input: alloy_primitives::Bytes::new(),
        };
        let signature = alloy_primitives::Signature::test_signature();
        let tx = TransactionSigned::new_unhashed(tx_inner.into(), signature);

        let mock_log = alloy_primitives::Log {
            address: alloy_primitives::Address::ZERO,
            data: alloy_primitives::LogData::new_unchecked(vec![], alloy_primitives::Bytes::new()),
        };

        let receipt = reth_ethereum_primitives::Receipt {
            tx_type: TxType::Legacy,
            cumulative_gas_used: 21_000,
            logs: vec![mock_log],
            success: true,
        };

        let mut prev_hash = alloy_primitives::B256::default();
        for (idx, block_number) in (100u64..=102).enumerate() {
            let header = alloy_consensus::Header {
                number: block_number,
                parent_hash: prev_hash,
                logs_bloom: Bloom::repeat_byte(1u8),
                ..Default::default()
            };
            let hash = header.hash_slow();
            prev_hash = hash;

            let block = reth_ethereum_primitives::Block {
                header,
                body: reth_ethereum_primitives::BlockBody {
                    transactions: vec![tx.clone()],
                    ..Default::default()
                },
            };
            provider.add_block(hash, block);
            provider.add_receipts(block_number, vec![receipt.clone()]);
            provider.add_block_body_indices(
                block_number,
                StoredBlockBodyIndices { first_tx_num: idx as u64, tx_count: 1 },
            );
        }

        let eth_api = build_test_eth_api(provider);
        let eth_filter = EthFilter::new(eth_api, EthFilterConfig::default(), Runtime::test());
        let err = eth_filter
            .inner
            .clone()
            .get_logs_in_block_range(
                Filter::default(),
                100,
                102,
                QueryLimits { max_blocks_per_filter: None, max_logs_per_response: Some(2) },
            )
            .await
            .expect_err("range should exceed max logs");

        let EthFilterError::QueryExceedsMaxResults { max_logs, from_block, to_block } = err else {
            panic!("unexpected error: {err:?}");
        };

        assert_eq!(max_logs, 2);
        assert_eq!(from_block, 100);
        assert_eq!(to_block, 101);
    }

    #[tokio::test]
    async fn test_non_consecutive_headers_after_bloom_filter() {
        let provider = MockEthProvider::default();

        // Create 4 headers where only blocks 100 and 102 will match bloom filter
        let mut expected_hashes = vec![];
        let mut prev_hash = alloy_primitives::B256::default();

        // Create a transaction for blocks that will have receipts
        use alloy_consensus::TxLegacy;
        use reth_ethereum_primitives::{TransactionSigned, TxType};

        let tx_inner = TxLegacy {
            chain_id: Some(1),
            nonce: 0,
            gas_price: 21_000,
            gas_limit: 21_000,
            to: alloy_primitives::TxKind::Call(alloy_primitives::Address::ZERO),
            value: alloy_primitives::U256::ZERO,
            input: alloy_primitives::Bytes::new(),
        };
        let signature = alloy_primitives::Signature::test_signature();
        let tx = TransactionSigned::new_unhashed(tx_inner.into(), signature);

        for i in 100u64..=103 {
            let header = alloy_consensus::Header {
                number: i,
                parent_hash: prev_hash,
                // Set bloom to match filter only for blocks 100 and 102
                logs_bloom: if i == 100 || i == 102 {
                    Bloom::repeat_byte(1u8)
                } else {
                    alloy_primitives::Bloom::default()
                },
                ..Default::default()
            };

            let hash = header.hash_slow();
            expected_hashes.push(hash);
            prev_hash = hash;

            // Add transaction to blocks that will have receipts (100 and 102)
            let transactions = if i == 100 || i == 102 { vec![tx.clone()] } else { vec![] };

            let block = reth_ethereum_primitives::Block {
                header,
                body: reth_ethereum_primitives::BlockBody { transactions, ..Default::default() },
            };
            provider.add_block(hash, block);
        }

        // Add receipts with logs only to blocks that match bloom
        let mock_log = alloy_primitives::Log {
            address: alloy_primitives::Address::ZERO,
            data: alloy_primitives::LogData::new_unchecked(vec![], alloy_primitives::Bytes::new()),
        };

        let receipt = reth_ethereum_primitives::Receipt {
            tx_type: TxType::Legacy,
            cumulative_gas_used: 21_000,
            logs: vec![mock_log],
            success: true,
        };

        provider.add_receipts(100, vec![receipt.clone()]);
        provider.add_receipts(101, vec![]);
        provider.add_receipts(102, vec![receipt.clone()]);
        provider.add_receipts(103, vec![]);

        // Add block body indices for each block so receipts can be fetched
        use reth_db_api::models::StoredBlockBodyIndices;
        provider
            .add_block_body_indices(100, StoredBlockBodyIndices { first_tx_num: 0, tx_count: 1 });
        provider
            .add_block_body_indices(101, StoredBlockBodyIndices { first_tx_num: 1, tx_count: 0 });
        provider
            .add_block_body_indices(102, StoredBlockBodyIndices { first_tx_num: 1, tx_count: 1 });
        provider
            .add_block_body_indices(103, StoredBlockBodyIndices { first_tx_num: 2, tx_count: 0 });

        let eth_api = build_test_eth_api(provider);
        let eth_filter = EthFilter::new(eth_api, EthFilterConfig::default(), Runtime::test());

        // Use default filter which will match any non-empty bloom
        let filter = Filter::default();

        // Get logs in the range - this will trigger the bloom filtering
        let logs = eth_filter
            .inner
            .clone()
            .get_logs_in_block_range(filter, 100, 103, QueryLimits::default())
            .await
            .expect("should succeed");

        // We should get logs from blocks 100 and 102 only (bloom filtered)
        assert_eq!(logs.len(), 2);

        assert_eq!(logs[0].block_number, Some(100));
        assert_eq!(logs[1].block_number, Some(102));

        // Each block hash should be the hash of its own header, not derived from any other header
        assert_eq!(logs[0].block_hash, Some(expected_hashes[0])); // block 100
        assert_eq!(logs[1].block_hash, Some(expected_hashes[2])); // block 102
    }

    #[tokio::test]
    async fn test_range_scan_waits_for_blocking_io_permit() {
        use reth_rpc_eth_api::helpers::SpawnBlocking;

        let provider = MockEthProvider::default();
        let header = alloy_consensus::Header::default();
        provider.add_header(header.hash_slow(), header);
        provider.add_receipts(0, vec![]);
        let eth_api = build_test_eth_api(provider);

        // take every permit so the scan has to wait for one
        let guard = eth_api.blocking_io_task_guard().clone();
        let permits =
            guard.clone().acquire_many_owned(guard.available_permits() as u32).await.unwrap();

        let eth_filter = EthFilter::new(eth_api, EthFilterConfig::default(), Runtime::test());
        let scan = eth_filter.inner.clone().get_logs_in_block_range(
            Filter::default(),
            0,
            0,
            QueryLimits::default(),
        );
        tokio::pin!(scan);
        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut scan).await.is_err(),
            "scan must wait for a blocking IO permit"
        );

        drop(permits);
        assert!(scan.await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_range_scan_across_windows() {
        use alloy_consensus::TxLegacy;
        use alloy_primitives::{Address, Bloom, Bytes, Log, LogData, Signature};
        use reth_db_api::models::StoredBlockBodyIndices;
        use reth_ethereum_primitives::{Block, BlockBody, Receipt, TransactionSigned};

        let provider = MockEthProvider::default();
        let tx = TransactionSigned::new_unhashed(
            TxLegacy {
                chain_id: Some(1),
                gas_price: 21_000,
                gas_limit: 21_000,
                ..Default::default()
            }
            .into(),
            Signature::test_signature(),
        );
        let receipt = Receipt {
            tx_type: TxType::Legacy,
            cumulative_gas_used: 21_000,
            logs: vec![Log {
                address: Address::ZERO,
                data: LogData::new_unchecked(vec![], Bytes::new()),
            }],
            success: true,
        };

        // an empty filter matches every header, so every chunk is scanned in parallel while
        // only three blocks have logs to return
        let matching_blocks = [5u64, 1_200, 2_400];
        let mut parent_hash = FixedBytes::default();
        for number in 0..=2_500u64 {
            let matches = matching_blocks.contains(&number);
            let header = alloy_consensus::Header {
                number,
                parent_hash,
                logs_bloom: if matches { Bloom::repeat_byte(1u8) } else { Bloom::default() },
                ..Default::default()
            };
            parent_hash = header.hash_slow();
            let transactions = if matches { vec![tx.clone()] } else { Vec::new() };
            provider.add_block(
                parent_hash,
                Block { header, body: BlockBody { transactions, ..Default::default() } },
            );
            if matches {
                let tx_num = matching_blocks.iter().position(|b| *b == number).unwrap() as u64;
                provider.add_receipts(number, vec![receipt.clone()]);
                provider.add_block_body_indices(
                    number,
                    StoredBlockBodyIndices { first_tx_num: tx_num, tx_count: 1 },
                );
            } else {
                provider.add_receipts(number, vec![]);
            }
        }

        let eth_filter = EthFilter::new(
            build_test_eth_api(provider),
            EthFilterConfig::default(),
            Runtime::test(),
        );
        let logs = eth_filter
            .inner
            .clone()
            .get_logs_in_block_range(Filter::default(), 0, 2_500, QueryLimits::default())
            .await
            .unwrap();
        let blocks = logs.iter().map(|log| log.block_number.unwrap()).collect::<Vec<_>>();
        assert_eq!(blocks, matching_blocks);

        // the log limit ends the scan in the chunk that exceeds it and names the last block that
        // fit as the range to retry with
        let err = eth_filter
            .inner
            .clone()
            .get_logs_in_block_range(
                Filter::default(),
                0,
                2_500,
                QueryLimits { max_blocks_per_filter: None, max_logs_per_response: Some(1) },
            )
            .await
            .unwrap_err();
        assert!(
            matches!(
                err,
                EthFilterError::QueryExceedsMaxResults {
                    max_logs: 1,
                    from_block: 0,
                    to_block: 1_199
                }
            ),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn test_range_scan_chunk_logs() {
        use alloy_consensus::TxLegacy;
        use alloy_primitives::{Address, Bytes, Log, LogData, Signature};
        use reth_db_api::models::StoredBlockBodyIndices;
        use reth_ethereum_primitives::{Block, BlockBody, Receipt, TransactionSigned};

        let address = Address::repeat_byte(1);
        let log = Log { address, data: LogData::new_unchecked(vec![], Bytes::new()) };
        let receipt = |logs: Vec<Log>| Receipt {
            tx_type: TxType::Legacy,
            cumulative_gas_used: 21_000,
            logs,
            success: true,
        };
        let transactions = (0..3)
            .map(|nonce| {
                TransactionSigned::new_unhashed(
                    TxLegacy { chain_id: Some(1), nonce, gas_limit: 21_000, ..Default::default() }
                        .into(),
                    Signature::test_signature(),
                )
            })
            .collect::<Vec<_>>();
        let mut logs_bloom = Bloom::default();
        logs_bloom.accrue_log(&log);

        // block 10 has logs in two of its three receipts, block 20 matches the bloom but its
        // receipts were pruned, and both are far enough from the tip to be scanned in chunks
        let provider = MockEthProvider::default();
        let mut parent_hash = FixedBytes::default();
        for number in 0..=600u64 {
            let header = alloy_consensus::Header {
                number,
                parent_hash,
                logs_bloom: if number == 10 || number == 20 {
                    logs_bloom
                } else {
                    Bloom::default()
                },
                ..Default::default()
            };
            parent_hash = header.hash_slow();
            if number == 10 {
                provider.add_block(
                    parent_hash,
                    Block {
                        header,
                        body: BlockBody {
                            transactions: transactions.clone(),
                            ..Default::default()
                        },
                    },
                );
                provider.add_receipts(
                    10,
                    vec![
                        receipt(vec![log.clone()]),
                        receipt(vec![]),
                        receipt(vec![log.clone(); 2]),
                    ],
                );
                provider.add_block_body_indices(
                    10,
                    StoredBlockBodyIndices { first_tx_num: 0, tx_count: 3 },
                );
            } else {
                provider.add_header(parent_hash, header);
            }
        }

        let eth_filter = EthFilter::new(
            build_test_eth_api(provider),
            EthFilterConfig::default(),
            Runtime::test(),
        );
        let filter = Filter::new().address(address);
        let logs = eth_filter
            .inner
            .clone()
            .get_logs_in_block_range(filter.clone(), 0, 19, QueryLimits::default())
            .await
            .unwrap();
        assert_eq!(
            logs.iter()
                .map(|log| (
                    log.block_number.unwrap(),
                    log.transaction_hash.unwrap(),
                    log.transaction_index.unwrap(),
                    log.log_index.unwrap()
                ))
                .collect::<Vec<_>>(),
            [
                (10, *transactions[0].tx_hash(), 0, 0),
                (10, *transactions[2].tx_hash(), 2, 1),
                (10, *transactions[2].tx_hash(), 2, 2),
            ]
        );

        let err = eth_filter
            .inner
            .clone()
            .get_logs_in_block_range(filter.clone(), 0, 100, QueryLimits::default())
            .await
            .unwrap_err();
        assert!(matches!(err, EthFilterError::ReceiptsUnavailable(20)), "{err:?}");

        // the log limit is exceeded before the scan of the same chunk fails
        let err = eth_filter
            .inner
            .clone()
            .get_logs_in_block_range(
                filter,
                5,
                100,
                QueryLimits { max_blocks_per_filter: None, max_logs_per_response: Some(2) },
            )
            .await
            .unwrap_err();
        assert!(
            matches!(
                err,
                EthFilterError::QueryExceedsMaxResults { max_logs: 2, from_block: 5, to_block: 9 }
            ),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn test_logs_for_filter_pending_to_block_ends_at_head() {
        let provider = MockEthProvider::default();
        let header = alloy_consensus::Header { number: 2, ..Default::default() };
        let hash = header.hash_slow();
        provider.add_header(hash, header);
        provider.add_receipts(2, vec![]);
        // the engine holds a payload that is not canonical yet
        provider.set_pending_block_num_hash(Some(alloy_eips::BlockNumHash::new(3, hash)));

        let eth_filter = EthFilter::new(
            build_test_eth_api(provider),
            EthFilterConfig::default(),
            Runtime::test(),
        );
        for filter in [
            Filter::new().from_block(0u64).to_block(BlockNumberOrTag::Pending),
            Filter::new().select(BlockNumberOrTag::Pending..),
        ] {
            let logs = eth_filter
                .inner
                .clone()
                .logs_for_filter(filter, QueryLimits::default())
                .await
                .unwrap();
            assert!(logs.is_empty());
        }
    }

    #[tokio::test]
    async fn test_logs_for_filter_over_pruned_receipts() {
        let provider = MockEthProvider::default();
        let mut parent_hash = FixedBytes::default();
        for number in 0..=2u64 {
            let header = alloy_consensus::Header {
                number,
                parent_hash,
                logs_bloom: Bloom::repeat_byte(1u8),
                ..Default::default()
            };
            parent_hash = header.hash_slow();
            provider.add_block(
                parent_hash,
                reth_ethereum_primitives::Block { header, body: Default::default() },
            );
            // the receipts of block 1 were pruned
            if number != 1 {
                provider.add_receipts(number, vec![]);
            }
        }

        let eth_filter = EthFilter::new(
            build_test_eth_api(provider),
            EthFilterConfig::default(),
            Runtime::test(),
        );

        // a range that reaches the pruned block is rejected instead of served incompletely
        let err = eth_filter
            .inner
            .clone()
            .logs_for_filter(Filter::new().from_block(0u64).to_block(2u64), QueryLimits::default())
            .await
            .unwrap_err();
        assert!(matches!(err, EthFilterError::ReceiptsUnavailable(1)), "{err:?}");

        // a range that avoids it is served
        let logs = eth_filter
            .inner
            .clone()
            .logs_for_filter(Filter::new().from_block(2u64).to_block(2u64), QueryLimits::default())
            .await
            .unwrap();
        assert!(logs.is_empty());
    }
}
