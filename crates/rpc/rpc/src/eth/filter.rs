//! `eth_` `Filter` RPC handler implementation

use alloy_consensus::{transaction::TxHashRef, BlockHeader, TxReceipt};
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
use itertools::Itertools;
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
    ProviderReceipt, ReceiptProvider, TransactionsProvider,
};
use reth_tasks::Runtime;
use reth_transaction_pool::{PoolTransaction, TransactionPool, ValidPoolTransaction};
use std::{
    collections::{HashMap, VecDeque},
    fmt,
    iter::{Peekable, StepBy},
    ops::RangeInclusive,
    pin::Pin,
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};
use tokio::{
    sync::{
        broadcast::{self, error::TryRecvError},
        oneshot, Mutex,
    },
    time::MissedTickBehavior,
};
use tokio_stream::{wrappers::ReceiverStream, Stream};
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

/// The JSON-RPC error code for exceeding a limit, as defined by EIP-1474.
const LIMIT_EXCEEDED_CODE: i32 = -32005;

/// How many times per `stale_filter_ttl` expired filters are evicted.
///
/// Expired filters count against the active filter limit until a sweep removes them, so this
/// bounds how long past its TTL an abandoned filter can hold a slot.
const STALE_FILTER_SWEEPS_PER_TTL: u32 = 4;

/// Upper bound on the capacity of a [`TxFeed`].
///
/// Unlike the pool's listener channels, a broadcast channel allocates all of its slots up front, so
/// following an arbitrarily large configured listener buffer could allocate gigabytes for the first
/// pending filter.
const MAX_TX_FEED_CAPACITY: usize = 1 << 16;

/// Threshold for deciding between cached and range mode processing
const CACHED_MODE_BLOCK_THRESHOLD: u64 = 250;

/// Threshold for bloom filter matches that triggers reduced caching
const HIGH_BLOOM_MATCH_THRESHOLD: usize = 20;

/// Threshold for bloom filter matches that triggers moderately reduced caching
const MODERATE_BLOOM_MATCH_THRESHOLD: usize = 10;

/// Minimum block count to apply bloom filter match adjustments
const BLOOM_ADJUSTMENT_MIN_BLOCKS: u64 = 100;

/// The maximum number of headers we read at once when handling a range filter.
const MAX_HEADERS_RANGE: u64 = 1_000; // with ~530bytes per header this is ~500kb

// Cached mode is only reachable for ranges that fit into a single header window, which is what
// keeps the mode decision independent of how a range is split.
const _: () = assert!(CACHED_MODE_BLOCK_THRESHOLD <= MAX_HEADERS_RANGE);

/// Minimum number of bloom matching blocks in a header window for fetching their receipts in
/// parallel
const PARALLEL_PROCESSING_THRESHOLD: usize = 1000;

/// Default concurrency for parallel processing
const DEFAULT_PARALLEL_CONCURRENCY: usize = 4;

/// Maximum number of blocks whose receipts the parallel fetching holds in memory at once
const MAX_PARALLEL_BATCH_SIZE: usize = 256;

/// Lower bound on the JSON size in bytes of a log without its topics and data: the field names,
/// the address and the block and transaction hashes.
const MIN_LOG_JSON_SIZE: usize = 300;

/// Upper bound on the JSON size in bytes of a log without the hex digits of its topics and data:
/// every quantity at its widest, the quotes, prefixes and separators of four topics, and the
/// separator to the next log.
const MAX_LOG_JSON_SIZE: usize = 436;

/// Allowance in bytes for the JSON-RPC envelope around the logs of a response, enough for request
/// ids of up to 90 bytes.
const MAX_RESPONSE_ENVELOPE_SIZE: usize = 128;

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
        let EthFilterConfig {
            max_blocks_per_filter,
            max_logs_per_response,
            stale_filter_ttl,
            max_response_size,
            max_active_filters,
        } = config;
        let inner = EthFilterInner {
            eth_api,
            active_filters: ActiveFilters::new(),
            max_active_filters,
            pending_transaction_hashes: Arc::new(TxFeed::new()),
            full_pending_transactions: OnceLock::new(),
            id_provider: Arc::new(EthSubscriptionIdProvider::default()),
            max_headers_range: MAX_HEADERS_RANGE,
            task_spawner,
            stale_filter_ttl,
            query_limits: QueryLimits {
                max_blocks_per_filter,
                max_logs_per_response,
                max_response_size,
            },
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

    /// Endless future that [`Self::clear_stale_filters`] [`STALE_FILTER_SWEEPS_PER_TTL`] times per
    /// `stale_filter_ttl`.
    /// Nonetheless, this endless future frees the thread at every await point.
    async fn watch_and_clear_stale_filters(&self) {
        // `interval_at` panics on a zero period, which a tiny TTL truncates to, and the timer does
        // not resolve below a millisecond.
        let period = (self.inner.stale_filter_ttl / STALE_FILTER_SWEEPS_PER_TTL)
            .max(Duration::from_millis(1));
        let mut interval = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
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
        let best_number = self.provider().chain_info()?.best_number;

        // start_block is the first block the filter has not delivered changes for yet
        let (start_block, kind) = {
            let mut filters = self.inner.active_filters.inner.lock().await;
            let filter =
                filters.get_mut(&id).ok_or_else(|| EthFilterError::FilterNotFound(id.clone()))?;
            filter.last_poll_timestamp = Instant::now();

            // Pending transactions arrive between blocks, so only block and log filters can skip a
            // poll without a new block.
            if filter.block > best_number &&
                matches!(filter.kind, FilterKind::Block | FilterKind::Log(_))
            {
                // no new blocks since the last poll
                return Ok(FilterChanges::Empty)
            }

            (filter.block, filter.kind.clone())
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
                self.advance_filter_cursor(&id, end_block).await;
                Ok(FilterChanges::Hashes(block_hashes))
            }
            FilterKind::Log(filter) => {
                let mut limits = self.inner.query_limits;
                // A poll acknowledges what it returns, so it only returns logs that fit the
                // response size limit at their largest encoding. The scan checks a lower bound,
                // which is at least `MIN_LOG_JSON_SIZE / MAX_LOG_JSON_SIZE` of that.
                limits.max_response_size = limits.max_response_size.map(|max| {
                    max.saturating_sub(MAX_RESPONSE_ENVELOPE_SIZE) / MAX_LOG_JSON_SIZE *
                        MIN_LOG_JSON_SIZE
                });
                let (from_block_number, to_block_number) = match filter.block_option {
                    FilterBlockOption::Range { from_block, to_block } => {
                        // `latest` and `pending` don't bound a poll, it covers the blocks since
                        // the last poll
                        let bound = |num: Option<BlockNumberOrTag>| {
                            num.filter(|num| !num.is_latest() && !num.is_pending())
                                .map(|num| self.provider().convert_block_number(num))
                                .transpose()
                                .map(Option::flatten)
                        };
                        let (from, to) = (bound(from_block)?, bound(to_block)?);
                        // a filter whose own range is invalid keeps reporting that, it is not a
                        // range the cursor has exhausted
                        if let (Some(from), Some(to)) = (from, to) &&
                            from > to
                        {
                            return Err(EthFilterError::InvalidBlockRangeParams)
                        }

                        // only the blocks since the last poll, clamped to the filter's own range
                        (
                            from.map_or(start_block, |from| from.max(start_block)),
                            to.map_or(best_number, |to| to.min(best_number)),
                        )
                    }
                    FilterBlockOption::AtBlockHash(_) => {
                        // The block is read by its hash, so a reorg cannot make the poll return the
                        // logs of the block that replaced it. Once delivered, no later poll has
                        // anything to return, since the block never changes.
                        let logs = self.inner.clone().logs_for_filter(*filter, limits).await?;
                        self.advance_filter_cursor(&id, u64::MAX).await;
                        return Ok(FilterChanges::Logs(logs))
                    }
                };
                if from_block_number > to_block_number {
                    // the filter's range contains no block since the last poll
                    return Ok(FilterChanges::Empty)
                }

                // a poll delivers at most the configured block cap, the next poll continues from
                // there
                let to_block_number = limits.max_blocks_per_filter.map_or(to_block_number, |max| {
                    to_block_number.min(from_block_number.saturating_add(max))
                });
                let (logs, delivered_to) = match self
                    .inner
                    .clone()
                    .get_logs_in_block_range(
                        *filter.clone(),
                        from_block_number,
                        to_block_number,
                        limits,
                    )
                    .await
                {
                    Ok(logs) => (logs, to_block_number),
                    Err(
                        EthFilterError::QueryExceedsMaxResults { to_block, .. } |
                        EthFilterError::QueryExceedsMaxResponseSize { to_block, .. },
                    ) => {
                        // deliver the prefix of blocks that fits under the log cap and the
                        // response size, the next poll continues from there
                        let logs = self
                            .inner
                            .clone()
                            .get_logs_in_block_range(*filter, from_block_number, to_block, limits)
                            .await?;
                        (logs, to_block)
                    }
                    Err(err @ EthFilterError::ReceiptsUnavailable(_)) => {
                        // Pruned receipts do not come back, so a retry would fail the same way on
                        // every later poll. The error reports the gap and the next poll continues
                        // after this poll's range.
                        self.advance_filter_cursor(&id, to_block_number + 1).await;
                        return Err(err)
                    }
                    Err(err) => return Err(err),
                };
                self.advance_filter_cursor(&id, delivered_to + 1).await;
                Ok(FilterChanges::Logs(logs))
            }
        }
    }

    /// Moves the cursor of the filter with the given id to `next`, the first block a subsequent
    /// poll returns changes for, or `u64::MAX` once the filter has nothing left to deliver.
    ///
    /// This runs after the changes were fetched so that a failed poll is retried by the next one.
    /// A poll therefore delivers its range at least once: two polls of the same filter that run
    /// concurrently both cover the range neither of them has acknowledged yet.
    ///
    /// There are two exceptions. A single block whose logs alone exceed the response size limit is
    /// returned whole, as `eth_getLogs` does for one block, so the cursor moves past it although
    /// the transport may reject the response. A poll that fails because receipts in its range were
    /// pruned moves the cursor past that range, since every retry would fail the same way.
    async fn advance_filter_cursor(&self, id: &FilterId, next: u64) {
        let mut filters = self.inner.active_filters.inner.lock().await;
        if let Some(filter) = filters.get_mut(id) {
            filter.block = filter.block.max(next);
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

impl<Eth> EthFilterApiServer<RpcTransaction<Eth::NetworkTypes>, RpcLog<Eth::NetworkTypes>>
    for EthFilter<Eth>
where
    Eth: FullEthApiTypes + LoadReceipt + EthBlocks,
{
    /// Handler for `eth_newFilter`
    async fn new_filter(&self, filter: Filter) -> RpcResult<FilterId> {
        trace!(target: "rpc::eth", "Serving eth_newFilter");
        self.inner.install_filter(|| FilterKind::Log(Box::new(filter))).await
    }

    /// Handler for `eth_newBlockFilter`
    async fn new_block_filter(&self) -> RpcResult<FilterId> {
        trace!(target: "rpc::eth", "Serving eth_newBlockFilter");
        self.inner.install_filter(|| FilterKind::Block).await
    }

    /// Handler for `eth_newPendingTransactionFilter`
    async fn new_pending_transaction_filter(
        &self,
        kind: Option<PendingTransactionFilterKind>,
    ) -> RpcResult<FilterId> {
        trace!(target: "rpc::eth", "Serving eth_newPendingTransactionFilter");

        let kind = kind.unwrap_or_default();

        // The filter only subscribes to its feed once it is within the filter limit, so a rejected
        // request neither starts a feed nor leaves a receiver behind.
        self.inner
            .install_filter(|| {
                FilterKind::PendingTransaction(match kind {
                    PendingTransactionFilterKind::Hashes => {
                        PendingTransactionKind::Hashes(PendingTransactionsReceiver::new(
                            self.inner.pending_transaction_hashes.subscribe(
                                &self.inner.task_spawner,
                                || {
                                    let listener = self.pool().pending_transactions_listener();
                                    let capacity = listener.max_capacity();
                                    (ReceiverStream::new(listener), capacity)
                                },
                            ),
                        ))
                    }
                    PendingTransactionFilterKind::Full => {
                        let feed = self.inner.full_pending_transactions.get_or_init(|| {
                            Arc::new(PoolFullTransactionsFeed::new(
                                self.pool().clone(),
                                dyn_clone::clone(self.inner.eth_api.converter()),
                            ))
                        });
                        PendingTransactionKind::FullTransaction(
                            feed.subscribe(&self.inner.task_spawner),
                        )
                    }
                })
            })
            .await
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
    /// Maximum number of installed filters, `None` if unlimited.
    max_active_filters: Option<usize>,
    /// The hashes of new pending transactions, shared by all pending transaction hash filters.
    pending_transaction_hashes: Arc<TxFeed<TxHash>>,
    /// The new pending transactions, shared by all full pending transaction filters.
    ///
    /// Filled by the first such filter, because the pool's transaction type is not known to this
    /// type.
    full_pending_transactions:
        OnceLock<Arc<dyn FullTransactionsFeed<RpcTransaction<Eth::NetworkTypes>>>>,
    /// Provides ids to identify filters
    id_provider: Arc<dyn IdProvider>,
    /// limits for logs queries
    query_limits: QueryLimits,
    /// maximum number of headers to read at once for range filter
    max_headers_range: u64,
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
                // Only take what the cache already holds, so that the receipts of a block whose
                // bloom rules out the filter are never read.
                let (maybe_block, receipts) =
                    self.eth_cache().maybe_cached_block_and_receipts(block_hash).await?;

                // With the block in the cache, serving its logs needs no further reads besides
                // the receipts, which the cache reads on its own blocking tasks.
                if let Some(block) = maybe_block {
                    let header = block.clone_sealed_header();
                    if !Self::block_logs_may_match(self.provider(), &filter, &header)? {
                        return Ok(Vec::new())
                    }
                    let receipts = match receipts {
                        Some(receipts) => receipts,
                        None => self
                            .eth_cache()
                            .get_receipts(block_hash)
                            .await?
                            .ok_or_else(|| Self::missing_receipts(self.provider(), &header))?,
                    };

                    let mut all_logs = Vec::new();
                    append_matching_block_logs(
                        &mut all_logs,
                        self.eth_api.converter(),
                        ProviderOrBlock::<Eth::Provider>::Block(block),
                        &filter,
                        &header,
                        &receipts,
                        false,
                    )?;
                    return Ok(all_logs)
                }

                // Everything else is read from the provider on one blocking task, which shares the
                // budget for blocking IO requests with the range scans instead of blocking an
                // executor thread.
                let cache = self.eth_cache().clone();
                self.spawn_blocking_io_with_permit(move |eth_api| {
                    let provider = eth_api.provider();
                    let header = provider
                        .sealed_header_by_hash(block_hash)?
                        .ok_or(ProviderError::HeaderNotFound(block_hash.into()))?;
                    if !Self::block_logs_may_match(provider, &filter, &header)? {
                        return Ok(Vec::new())
                    }
                    let receipts = match receipts {
                        Some(receipts) => receipts,
                        None => {
                            let receipts = Arc::new(
                                provider
                                    .receipts_by_block(block_hash.into())?
                                    .ok_or_else(|| Self::missing_receipts(provider, &header))?,
                            );
                            // Kept for later queries of this block, as a read through the cache
                            // would.
                            cache.insert_receipts(block_hash, receipts.clone());
                            receipts
                        }
                    };
                    Self::block_logs_from_provider(&eth_api, &filter, &header, &receipts)
                })
                .await
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

    /// Installs the filter that `kind` returns and returns the new identifier.
    ///
    /// Returns [`EthFilterError::TooManyFilters`] without calling `kind` if the maximum number of
    /// active filters is reached. The limit check and the insertion happen under the same lock, so
    /// concurrent requests cannot exceed it, and `kind` runs under that lock, so it must not wait.
    async fn install_filter(
        &self,
        kind: impl FnOnce() -> FilterKind<RpcTransaction<Eth::NetworkTypes>>,
    ) -> RpcResult<FilterId> {
        let last_poll_block_number = self.provider().best_block_number().to_rpc_result()?;
        let subscription_id = self.id_provider.next_id();

        let id = match subscription_id {
            jsonrpsee_types::SubscriptionId::Num(n) => FilterId::Num(n),
            jsonrpsee_types::SubscriptionId::Str(s) => FilterId::Str(s.into_owned()),
        };
        let mut filters = self.active_filters.inner.lock().await;
        if let Some(max) = self.max_active_filters &&
            filters.len() >= max
        {
            return Err(EthFilterError::TooManyFilters(max).into())
        }
        filters.insert(
            id.clone(),
            ActiveFilter {
                block: last_poll_block_number,
                last_poll_timestamp: Instant::now(),
                kind: kind(),
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
        // A lower bound on the JSON size of the matching logs.
        let mut response_size = 0;
        let mut retry_range_end = to_block;

        // get current chain tip to determine processing mode
        let chain_tip = self.provider().best_block_number()?;

        // Scan the range window by window so that receipts are fetched while headers are still
        // being read: the log limit can end the query after the first window, memory is bounded by
        // one window, and a cancelled query stops at the next window.
        for (from, to) in
            BlockRangeInclusiveIter::new(from_block..=to_block, self.max_headers_range)
        {
            // reading headers is blocking, this gives the cancellation check a chance to run
            tokio::task::yield_now().await;

            // collect the headers of this window that match the bloom filter
            let mut matching_headers = Vec::new();
            let headers = self.provider().headers_range(from..=to)?;

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

            // initialize the appropriate range mode based on collected headers
            let mut range_mode = RangeMode::new(
                self.clone(),
                matching_headers,
                from_block,
                to_block,
                self.max_headers_range,
                chain_tip,
            );

            // iterate through the range mode to get receipts and blocks
            while let Some(ReceiptBlockResult { receipts, recovered_block, header }) =
                range_mode.next().await?
            {
                let num_hash = header.num_hash();
                let logs_before = all_logs.len();

                let payload_len = append_matching_block_logs(
                    &mut all_logs,
                    self.eth_api.converter(),
                    recovered_block
                        .map(ProviderOrBlock::Block)
                        .unwrap_or_else(|| ProviderOrBlock::Provider(self.provider())),
                    filter,
                    &header,
                    &receipts,
                    false,
                )?;

                // size check but only if range is multiple blocks, so we always return all
                // logs of a single block
                let is_multi_block_range = from_block != to_block;
                if let Some(max_response_size) = limits.max_response_size &&
                    is_multi_block_range
                {
                    // Topics and data are hex encoded.
                    response_size +=
                        (all_logs.len() - logs_before) * MIN_LOG_JSON_SIZE + payload_len * 2;
                    // The transport measures the encoded response, so the retry range ends before
                    // the first block whose logs may no longer fit it. A range of only
                    // `from_block` is left to the transport, like any single block.
                    let max_encoded_size = MAX_RESPONSE_ENVELOPE_SIZE +
                        response_size +
                        all_logs.len() * (MAX_LOG_JSON_SIZE - MIN_LOG_JSON_SIZE);
                    if max_encoded_size > max_response_size {
                        retry_range_end =
                            retry_range_end.min(num_hash.number.saturating_sub(1).max(from_block));
                    }
                    if response_size > max_response_size {
                        debug!(
                            target: "rpc::eth::filter",
                            response_size,
                            max_response_size,
                            from_block,
                            to_block = retry_range_end,
                            "Query exceeded max response size"
                        );
                        return Err(EthFilterError::QueryExceedsMaxResponseSize {
                            max_bytes: max_response_size,
                            from_block,
                            to_block: retry_range_end,
                        });
                    }
                }

                if let Some(max_logs_per_response) = limits.max_logs_per_response &&
                    is_multi_block_range &&
                    all_logs.len() > max_logs_per_response
                {
                    let retry_to_block = if num_hash.number == from_block {
                        from_block
                    } else {
                        num_hash.number - 1
                    };

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
        }

        Ok(all_logs)
    }

    /// Runs `f` on a blocking task once a blocking IO permit is available, holding the permit until
    /// `f` returns.
    async fn spawn_blocking_io_with_permit<R: Send + 'static>(
        &self,
        f: impl FnOnce(Eth) -> Result<R, EthFilterError> + Send + 'static,
    ) -> Result<R, EthFilterError> {
        let permit = self
            .eth_api
            .acquire_owned_blocking_io()
            .await
            .map_err(|_| EthFilterError::InternalError)?;
        self.eth_api
            .spawn_blocking_io(move |eth_api| {
                let _permit = permit;
                Ok(f(eth_api))
            })
            .await
            .map_err(|_| EthFilterError::InternalError)?
    }

    /// Returns the logs of the block with the given header that match the filter, reading the
    /// transactions of the receipts with matching logs from the provider.
    ///
    /// This performs blocking reads and must run on a blocking task.
    fn block_logs_from_provider(
        eth_api: &Eth,
        filter: &Filter,
        header: &SealedHeader<<Eth::Provider as HeaderProvider>::Header>,
        receipts: &[ProviderReceipt<Eth::Provider>],
    ) -> Result<Vec<RpcLog<Eth::NetworkTypes>>, EthFilterError> {
        let provider = eth_api.provider();
        let mut all_logs = Vec::new();

        // Looking up a transaction by its id costs about as much as decoding four of the block's
        // transactions, so they are all read at once if more than a quarter of the receipts have
        // matching logs.
        let more_than_a_quarter_match = receipts
            .iter()
            .filter(|receipt| receipt.logs().iter().any(|log| filter.matches(log)))
            .nth(receipts.len() / 4)
            .is_some();
        if more_than_a_quarter_match {
            // Reading them by hash fails instead of returning the transactions of another block if
            // this one was reorged out in the meantime.
            let transactions = provider
                .transactions_by_block(header.hash().into())?
                .ok_or(ProviderError::HeaderNotFound(header.hash().into()))?;
            logs_utils::append_matching_block_logs_with(
                &mut all_logs,
                eth_api.converter(),
                filter,
                header,
                receipts,
                false,
                |receipt_idx| {
                    // A block's transactions and receipts share their indices.
                    let transaction =
                        transactions.get(receipt_idx).ok_or(EthApiError::TransactionNotFound)?;
                    Ok(Some(*transaction.tx_hash()))
                },
            )?;
            return Ok(all_logs)
        }

        let res = append_matching_block_logs(
            &mut all_logs,
            eth_api.converter(),
            ProviderOrBlock::Provider(provider),
            filter,
            header,
            receipts,
            false,
        );

        // The transactions were looked up by the number of the block, so they are its own only if
        // it is still the canonical block at that number. Numbers resolve to the in-memory
        // canonical chain before the database, as those lookups do, so this also catches a reorg
        // the database has not applied yet. A reorg also explains a failed lookup.
        if (res.is_err() || !all_logs.is_empty()) &&
            provider.block_hash(header.number())? != Some(header.hash())
        {
            return Err(ProviderError::HeaderNotFound(header.hash().into()).into())
        }
        res?;
        Ok(all_logs)
    }

    /// Returns whether the bloom of the block allows logs matching the filter.
    ///
    /// Returns an error if the history of the block has expired (EIP-4444).
    fn block_logs_may_match(
        provider: &Eth::Provider,
        filter: &Filter,
        header: &SealedHeader<<Eth::Provider as HeaderProvider>::Header>,
    ) -> Result<bool, EthFilterError> {
        let earliest_block = provider.earliest_block_number()?;
        if header.number() < earliest_block {
            return Err(EthApiError::PrunedHistoryUnavailable {
                requested: header.number(),
                earliest_available: earliest_block,
            }
            .into());
        }

        Ok(filter.matches_bloom(header.logs_bloom()))
    }

    /// Returns the error for a block whose receipts cannot be read although its header was found.
    ///
    /// The receipts were pruned, unless the block was reorged out after its header was read.
    fn missing_receipts(
        provider: &Eth::Provider,
        header: &SealedHeader<<Eth::Provider as HeaderProvider>::Header>,
    ) -> EthFilterError {
        match provider.block_number(header.hash()) {
            Ok(Some(_)) => EthFilterError::ReceiptsUnavailable(header.number()),
            Ok(None) => ProviderError::HeaderNotFound(header.hash().into()).into(),
            Err(err) => err.into(),
        }
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
    /// The first block the next poll returns changes for, or `u64::MAX` once a log filter has
    /// nothing left to deliver.
    block: u64,
    /// Last time this filter was polled.
    last_poll_timestamp: Instant,
    /// What kind of filter it is.
    kind: FilterKind<T>,
}

/// A receiver of the shared feed of pending transaction hashes.
///
/// Returns all hashes that were published since the last poll.
#[derive(Debug, Clone)]
struct PendingTransactionsReceiver {
    txs_receiver: Arc<Mutex<broadcast::Receiver<TxHash>>>,
}

impl PendingTransactionsReceiver {
    fn new(receiver: broadcast::Receiver<TxHash>) -> Self {
        Self { txs_receiver: Arc::new(Mutex::new(receiver)) }
    }

    /// Returns all new pending transactions received since the last poll.
    async fn drain<T>(&self) -> FilterChanges<T> {
        let mut pending_txs = Vec::new();
        drain_feed(&mut *self.txs_receiver.lock().await, |tx_hash| pending_txs.push(tx_hash));
        FilterChanges::Hashes(pending_txs)
    }
}

/// A receiver of the shared feed of new pending transactions, that returns them as RPC
/// transactions.
#[derive(Debug)]
struct FullTransactionsReceiver<T: PoolTransaction, TxCompat> {
    txs_receiver: Mutex<broadcast::Receiver<Arc<ValidPoolTransaction<T>>>>,
    converter: TxCompat,
}

impl<T, TxCompat> FullTransactionsReceiver<T, TxCompat>
where
    T: PoolTransaction + 'static,
    TxCompat: RpcConvert<Primitives: NodePrimitives<SignedTx = T::Consensus>>,
{
    /// Returns all new pending transactions received since the last poll.
    async fn drain(&self) -> FilterChanges<RpcTransaction<TxCompat::Network>> {
        // The guard is dropped before the conversion, so a concurrent poll of the filter only waits
        // for the transactions to be taken from the feed.
        let mut txs = Vec::new();
        drain_feed(&mut *self.txs_receiver.lock().await, |tx| txs.push(tx));
        let pending_txs = txs
            .into_iter()
            .filter_map(|tx| match self.converter.fill_pending(tx.to_consensus()) {
                Ok(tx) => Some(tx),
                Err(err) => {
                    error!(target: "rpc",
                        %err,
                        "Failed to fill txn with block context"
                    );
                    None
                }
            })
            .collect();
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

/// The shared feed of new pending transactions behind all full pending transaction filters.
#[derive(Debug)]
struct PoolFullTransactionsFeed<Pool: TransactionPool, TxCompat> {
    pool: Pool,
    converter: TxCompat,
    feed: Arc<TxFeed<Arc<ValidPoolTransaction<Pool::Transaction>>>>,
}

impl<Pool, TxCompat> PoolFullTransactionsFeed<Pool, TxCompat>
where
    Pool: TransactionPool,
{
    /// Creates a feed without a producer.
    fn new(pool: Pool, converter: TxCompat) -> Self {
        Self { pool, converter, feed: Arc::new(TxFeed::new()) }
    }
}

/// Helper trait for [`PoolFullTransactionsFeed`] to erase the pool type.
trait FullTransactionsFeed<T>: fmt::Debug + Send + Sync + 'static {
    /// Returns a filter over the feed that only sees the transactions published from now on.
    ///
    /// Starts the producer on `spawner` if the feed has none.
    fn subscribe(&self, spawner: &Runtime) -> Arc<dyn FullTransactionsFilter<T>>;
}

impl<Pool, TxCompat> FullTransactionsFeed<RpcTransaction<TxCompat::Network>>
    for PoolFullTransactionsFeed<Pool, TxCompat>
where
    Pool: TransactionPool<
            Transaction: PoolTransaction<
                Consensus = <TxCompat::Primitives as NodePrimitives>::SignedTx,
            >,
        > + 'static,
    TxCompat: RpcConvert,
{
    fn subscribe(
        &self,
        spawner: &Runtime,
    ) -> Arc<dyn FullTransactionsFilter<RpcTransaction<TxCompat::Network>>> {
        let txs_receiver = self.feed.subscribe(spawner, || {
            let listener = self.pool.new_pending_pool_transactions_listener();
            let capacity = listener.max_capacity();
            (listener.map(|event| event.transaction), capacity)
        });
        Arc::new(FullTransactionsReceiver {
            txs_receiver: Mutex::new(txs_receiver),
            converter: dyn_clone::clone(&self.converter),
        })
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
    /// Returns the transactions published to the kind's feed since the last poll.
    ///
    /// A filter that is polled less often than its feed holds items loses the oldest ones, like a
    /// lagging subscription does.
    async fn drain(&self) -> FilterChanges<T> {
        match self {
            Self::Hashes(receiver) => receiver.drain().await,
            Self::FullTransaction(receiver) => receiver.drain().await,
        }
    }
}

/// Broadcasts the new pending transactions of one filter kind to all filters of that kind.
///
/// The pool buffers new transactions per listener, so a listener for every filter would multiply
/// that buffer and the pool's work per inserted transaction with the number of filters. All filters
/// of a kind share one listener instead. The feed holds as many transactions as that listener's
/// buffer, so a filter can fall as far behind as a listener of its own could before it loses the
/// oldest ones.
#[derive(Debug)]
struct TxFeed<T> {
    /// The running producer's sender, if any.
    ///
    /// The producer holds the only strong sender, so if it dies, its filters end and the next
    /// filter starts a new producer.
    sender: parking_lot::Mutex<Option<broadcast::WeakSender<T>>>,
}

impl<T: Clone + Send + 'static> TxFeed<T> {
    /// Creates a feed without a producer.
    const fn new() -> Self {
        Self { sender: parking_lot::Mutex::new(None) }
    }

    /// Subscribes to the feed. If no producer is running, this starts one on `spawner` over the
    /// stream that `source` returns, together with the capacity of the listener behind it.
    ///
    /// `source` runs before this returns, so the producer does not miss anything that is
    /// published after the call.
    fn subscribe<S>(
        self: &Arc<Self>,
        spawner: &Runtime,
        source: impl FnOnce() -> (S, usize),
    ) -> broadcast::Receiver<T>
    where
        S: Stream<Item = T> + Send + Unpin + 'static,
    {
        let mut slot = self.sender.lock();
        if let Some(sender) = slot.as_ref().and_then(broadcast::WeakSender::upgrade) {
            return sender.subscribe()
        }
        let (source, capacity) = source();
        let (sender, receiver) = broadcast::channel(capacity.min(MAX_TX_FEED_CAPACITY));
        *slot = Some(sender.downgrade());
        let feed = self.clone();
        spawner.spawn_task(async move { feed.produce(sender, source).await });
        receiver
    }

    /// Broadcasts the items of `source` until the feed has no receivers left or `source` ends.
    async fn produce<S: Stream<Item = T> + Unpin>(
        &self,
        sender: broadcast::Sender<T>,
        mut source: S,
    ) {
        loop {
            tokio::select! {
                _ = sender.closed() => {
                    if self.stop_if_unused(&sender) {
                        break
                    }
                }
                item = source.next() => {
                    let Some(item) = item else {
                        // Dropping the sender ends all filters of the feed.
                        *self.sender.lock() = None;
                        break
                    };
                    if sender.send(item).is_err() && self.stop_if_unused(&sender) {
                        break
                    }
                }
            }
        }
        debug!(target: "rpc::eth::filter", "pending transaction feed stopped");
    }

    /// Removes the producer behind `sender` from the feed if it has no receivers left, and returns
    /// whether it did.
    ///
    /// Subscribing holds the same lock, so a new filter either joins before this check or starts a
    /// new producer after it.
    fn stop_if_unused(&self, sender: &broadcast::Sender<T>) -> bool {
        let mut slot = self.sender.lock();
        if sender.receiver_count() > 0 {
            return false
        }
        *slot = None;
        true
    }
}

/// Passes everything that `receiver` has buffered to `f`, oldest first.
///
/// If the receiver lagged behind its feed, this skips to the oldest item that the feed still
/// holds.
fn drain_feed<T: Clone>(receiver: &mut broadcast::Receiver<T>, mut f: impl FnMut(T)) {
    loop {
        match receiver.try_recv() {
            Ok(item) => f(item),
            Err(TryRecvError::Lagged(skipped)) => {
                debug!(target: "rpc::eth::filter", skipped, "pending transaction filter lagging behind its feed")
            }
            Err(TryRecvError::Empty | TryRecvError::Closed) => break,
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
    /// Query result exceeds the response size limit.
    #[error(
        "query exceeds max response size {max_bytes} bytes, retry with the range {from_block}-{to_block}"
    )]
    QueryExceedsMaxResponseSize {
        /// Maximum response size in bytes
        max_bytes: usize,
        /// Start block of the suggested retry range
        from_block: u64,
        /// End block of the suggested retry range (last block before the response may no longer
        /// fit)
        to_block: u64,
    },
    /// Error serving request in `eth_` namespace.
    #[error(transparent)]
    EthAPIError(#[from] EthApiError),
    /// Error thrown when a spawned task failed to deliver a response.
    #[error("internal filter error")]
    InternalError,
    /// The maximum number of active filters is reached.
    #[error("too many active filters, max {0}")]
    TooManyFilters(usize),
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
            err @ EthFilterError::TooManyFilters(_) => {
                rpc_error_with_code(LIMIT_EXCEEDED_CODE, err.to_string())
            }
            EthFilterError::EthAPIError(err) => err.into(),
            err @ EthFilterError::ReceiptsUnavailable(_) => {
                rpc_error_with_code(EthRpcErrorCode::PrunedHistory.code(), err.to_string())
            }
            err @ (EthFilterError::InvalidBlockRangeParams |
            EthFilterError::QueryExceedsMaxBlocks(_) |
            EthFilterError::QueryExceedsMaxResults { .. } |
            EthFilterError::QueryExceedsMaxResponseSize { .. } |
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
    /// Creates a new `RangeMode`.
    fn new(
        filter_inner: Arc<EthFilterInner<Eth>>,
        sealed_headers: Vec<SealedHeader<<Eth::Provider as HeaderProvider>::Header>>,
        from_block: u64,
        to_block: u64,
        max_headers_range: u64,
        chain_tip: u64,
    ) -> Self {
        let block_count = to_block - from_block + 1;
        let distance_from_tip = chain_tip.saturating_sub(to_block);

        // Determine if we should use cached mode based on range characteristics
        let use_cached_mode =
            Self::should_use_cached_mode(&sealed_headers, block_count, distance_from_tip);

        // Fetching receipts in parallel is only worth the extra tasks when most of the window has
        // them to read; the sequential path serves the rest from the receipt cache where it can
        let parallel = sealed_headers.len() >= PARALLEL_PROCESSING_THRESHOLD;

        if use_cached_mode && !sealed_headers.is_empty() {
            Self::Cached(CachedMode { filter_inner, headers_iter: sealed_headers.into_iter() })
        } else {
            Self::Range(RangeBlockMode {
                filter_inner,
                iter: sealed_headers.into_iter().peekable(),
                next: VecDeque::new(),
                max_range: (max_headers_range as usize).min(MAX_PARALLEL_BATCH_SIZE),
                parallel,
                pending_tasks: FuturesOrdered::new(),
            })
        }
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

    /// Gets the next (receipts, `maybe_block`, header, `block_hash`) tuple.
    async fn next(&mut self) -> Result<Option<ReceiptBlockResult<Eth::Provider>>, EthFilterError> {
        match self {
            Self::Cached(cached) => cached.next().await,
            Self::Range(range) => range.next().await,
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

/// Type alias for parallel receipt fetching task futures used in `RangeBlockMode`
type ReceiptFetchFuture<P> =
    Pin<Box<dyn Future<Output = Result<Vec<ReceiptBlockResult<P>>, EthFilterError>> + Send>>;

/// Mode for processing blocks using range queries for older blocks
struct RangeBlockMode<
    Eth: RpcNodeCoreExt<Provider: BlockIdReader, Pool: TransactionPool> + LoadReceipt + EthBlocks,
> {
    filter_inner: Arc<EthFilterInner<Eth>>,
    iter: Peekable<std::vec::IntoIter<SealedHeader<<Eth::Provider as HeaderProvider>::Header>>>,
    next: VecDeque<ReceiptBlockResult<Eth::Provider>>,
    /// Maximum number of consecutive blocks fetched by one batch of parallel tasks
    max_range: usize,
    /// Whether receipts are fetched in parallel batches of consecutive blocks
    parallel: bool,
    // Stream of ongoing receipt fetching tasks
    pending_tasks: FuturesOrdered<ReceiptFetchFuture<Eth::Provider>>,
}

impl<
        Eth: RpcNodeCoreExt<Provider: BlockIdReader, Pool: TransactionPool> + LoadReceipt + EthBlocks,
    > RangeBlockMode<Eth>
{
    async fn next(&mut self) -> Result<Option<ReceiptBlockResult<Eth::Provider>>, EthFilterError> {
        loop {
            // First, try to return any already processed result from buffer
            if let Some(result) = self.next.pop_front() {
                return Ok(Some(result));
            }

            // Try to get a completed task result if there are pending tasks
            if let Some(task_result) = self.pending_tasks.next().await {
                self.next.extend(task_result?);
                continue;
            }

            // No pending tasks - try to generate more work
            let Some(next_header) = self.iter.next() else {
                // No more headers to process
                return Ok(None);
            };

            if !self.parallel {
                // Process the block on its own so that only one block's receipts are held
                if let Some(result) = self.process_small_range(vec![next_header]).await? {
                    return Ok(Some(result));
                }
                // Continue loop to check for more work
                continue;
            }

            let mut range_headers = Vec::with_capacity(self.max_range.min(self.iter.len() + 1));
            range_headers.push(next_header);

            // Collect consecutive blocks up to max_range size
            while range_headers.len() < self.max_range {
                let Some(peeked) = self.iter.peek() else { break };
                let Some(last_header) = range_headers.last() else { break };

                let expected_next = last_header.number() + 1;
                if peeked.number() != expected_next {
                    trace!(
                        target: "rpc::eth::filter",
                        last_block = last_header.number(),
                        next_block = peeked.number(),
                        expected = expected_next,
                        range_size = range_headers.len(),
                        "Non-consecutive block detected, stopping range collection"
                    );
                    break; // Non-consecutive block, stop here
                }

                let Some(next_header) = self.iter.next() else { break };
                range_headers.push(next_header);
            }

            self.spawn_parallel_tasks(range_headers);
            // Continue loop to await the spawned tasks
        }
    }

    /// Process a small range of headers sequentially
    ///
    /// This is used for ranges below [`PARALLEL_PROCESSING_THRESHOLD`], one block at a time.
    async fn process_small_range(
        &mut self,
        range_headers: Vec<SealedHeader<<Eth::Provider as HeaderProvider>::Header>>,
    ) -> Result<Option<ReceiptBlockResult<Eth::Provider>>, EthFilterError> {
        // Process each header individually to avoid queuing for all receipts
        for header in range_headers {
            // First check if already cached to avoid unnecessary provider calls
            let (maybe_block, maybe_receipts) = self
                .filter_inner
                .eth_cache()
                .maybe_cached_block_and_receipts(header.hash())
                .await?;

            let receipts = match maybe_receipts {
                Some(receipts) => receipts,
                None => {
                    // Not cached - fetch directly from provider
                    match self.filter_inner.provider().receipts_by_block(header.hash().into())? {
                        Some(receipts) => Arc::new(receipts),
                        None => return Err(EthFilterError::ReceiptsUnavailable(header.number())),
                    }
                }
            };

            if !receipts.is_empty() {
                self.next.push_back(ReceiptBlockResult {
                    receipts,
                    recovered_block: maybe_block,
                    header,
                });
            }
        }

        Ok(self.next.pop_front())
    }

    /// Spawn parallel tasks for processing a large range of headers
    ///
    /// This is used for ranges of at least [`PARALLEL_PROCESSING_THRESHOLD`] blocks.
    fn spawn_parallel_tasks(
        &mut self,
        range_headers: Vec<SealedHeader<<Eth::Provider as HeaderProvider>::Header>>,
    ) {
        // Split headers into chunks
        let chunk_size = range_headers.len().div_ceil(DEFAULT_PARALLEL_CONCURRENCY).max(1);
        let header_chunks = range_headers
            .into_iter()
            .chunks(chunk_size)
            .into_iter()
            .map(|chunk| chunk.collect::<Vec<_>>())
            .collect::<Vec<_>>();

        // Spawn each chunk as a separate task directly into the FuturesOrdered stream
        for chunk_headers in header_chunks {
            let filter_inner = self.filter_inner.clone();
            let fetch = move || Self::fetch_chunk_receipts(&filter_inner, chunk_headers);

            // A parallel task occupies an additional blocking thread, so it needs its own share
            // of the blocking IO budget. Without one the chunk is fetched on this task instead.
            let chunk_task: ReceiptFetchFuture<Eth::Provider> = match self
                .filter_inner
                .eth_api
                .blocking_io_task_guard()
                .clone()
                .try_acquire_owned()
            {
                Ok(permit) => Box::pin(async move {
                    let chunk_task = reth_tasks::spawn_blocking(move || {
                        let _permit = permit;
                        fetch()
                    });

                    // Await the blocking task and handle the result
                    match chunk_task.await {
                        Ok(chunk_results) => chunk_results,
                        Err(join_err) => {
                            trace!(target: "rpc::eth::filter", error = ?join_err, "Task join error");
                            Err(EthFilterError::InternalError)
                        }
                    }
                }),
                Err(_) => Box::pin(async move { fetch() }),
            };

            self.pending_tasks.push_back(chunk_task);
        }
    }

    /// Fetches the receipts of the given blocks from the provider.
    fn fetch_chunk_receipts(
        filter_inner: &EthFilterInner<Eth>,
        chunk_headers: Vec<SealedHeader<<Eth::Provider as HeaderProvider>::Header>>,
    ) -> Result<Vec<ReceiptBlockResult<Eth::Provider>>, EthFilterError> {
        let mut chunk_results = Vec::with_capacity(chunk_headers.len());

        for header in chunk_headers {
            // Fetch directly from provider - RangeMode is used for older blocks
            // unlikely to be cached
            let receipts = match filter_inner.provider().receipts_by_block(header.hash().into())? {
                Some(receipts) => Arc::new(receipts),
                None => return Err(EthFilterError::ReceiptsUnavailable(header.number())),
            };

            if !receipts.is_empty() {
                chunk_results.push(ReceiptBlockResult { receipts, recovered_block: None, header });
            }
        }

        Ok(chunk_results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{eth::EthApi, EthApiBuilder};
    use alloy_consensus::TxLegacy;
    use alloy_eips::BlockId;
    use alloy_network::{Ethereum, TransactionResponse};
    use alloy_primitives::{Address, Bloom, Bytes, FixedBytes, Log, LogData, Signature, B256};
    use futures::stream;
    use jsonrpsee::{types::Id, MethodResponse, ResponsePayload};
    use rand::Rng;
    use reth_chainspec::{ChainSpec, ChainSpecProvider};
    use reth_db_api::models::StoredBlockBodyIndices;
    use reth_ethereum_primitives::{Block, BlockBody, Receipt, TransactionSigned, TxType};
    use reth_evm_ethereum::EthEvmConfig;
    use reth_network_api::noop::NoopNetwork;
    use reth_provider::test_utils::MockEthProvider;
    use reth_rpc_convert::RpcConverter;
    use reth_rpc_eth_api::{helpers::SpawnBlocking, node::RpcNodeCoreAdapter, RpcNodeCore};
    use reth_rpc_eth_types::receipt::EthReceiptConverter;
    use reth_tasks::Runtime;
    use reth_testing_utils::generators;
    use reth_transaction_pool::{
        test_utils::{testing_pool, MockTransaction, TestPool, TestPoolBuilder},
        PoolConfig, TransactionOrigin,
    };
    use std::{collections::VecDeque, sync::Arc};

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

    type TestEthApi = EthApi<
        RpcNodeCoreAdapter<MockEthProvider, TestPool, NoopNetwork, EthEvmConfig>,
        RpcConverter<Ethereum, EthEvmConfig, EthReceiptConverter<ChainSpec>>,
    >;

    // Helper function to create a test EthApi instance
    fn build_test_eth_api(provider: MockEthProvider) -> TestEthApi {
        build_test_eth_api_with_pool(provider, testing_pool())
    }

    fn build_test_eth_api_with_pool(provider: MockEthProvider, pool: TestPool) -> TestEthApi {
        EthApiBuilder::new(
            provider.clone(),
            pool,
            NoopNetwork::default(),
            EthEvmConfig::new(provider.chain_spec()),
        )
        .build()
    }

    /// Adds one block per number in `blocks`, each with a single legacy transaction whose receipt
    /// emits one log, so that a `Filter::default()` matches exactly one log per block.
    fn add_blocks_with_log(provider: &MockEthProvider, blocks: RangeInclusive<u64>) {
        add_blocks_with_logs(provider, blocks, 1)
    }

    /// Adds one block per number in `blocks`, each with a single legacy transaction whose receipt
    /// emits `logs_per_block` logs that a `Filter::default()` matches.
    ///
    /// The block body indices assume the added blocks form a range anchored at 0, which is what
    /// `MockEthProvider::transaction_by_id` resolves against.
    fn add_blocks_with_logs(
        provider: &MockEthProvider,
        blocks: RangeInclusive<u64>,
        logs_per_block: usize,
    ) {
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
        let log =
            Log { address: Address::ZERO, data: LogData::new_unchecked(vec![], Bytes::new()) };
        let receipt = Receipt::standard(TxType::Legacy, true, 21_000, vec![log; logs_per_block]);

        let mut parent_hash = blocks
            .start()
            .checked_sub(1)
            .and_then(|parent| provider.header_by_number(parent).unwrap())
            .map(|parent| parent.hash_slow())
            .unwrap_or_default();
        for number in blocks {
            let header = alloy_consensus::Header {
                number,
                parent_hash,
                logs_bloom: Bloom::from([1u8; 256]),
                ..Default::default()
            };
            let hash = header.hash_slow();
            let block = Block {
                header,
                body: BlockBody { transactions: vec![tx.clone()], ..Default::default() },
            };
            provider.add_block(hash, block);
            provider.add_receipts(number, vec![receipt.clone()]);
            provider.add_block_body_indices(
                number,
                StoredBlockBodyIndices { first_tx_num: number, tx_count: 1 },
            );
            parent_hash = hash;
        }
    }

    /// Polls the filter and returns the block numbers of the delivered logs.
    async fn poll_log_blocks<Eth>(eth_filter: &EthFilter<Eth>, id: &FilterId) -> Vec<u64>
    where
        Eth: FullEthApiTypes<Provider: BlockReader + BlockIdReader>
            + RpcNodeCoreExt
            + LoadReceipt
            + EthBlocks
            + 'static,
        RpcLog<Eth::NetworkTypes>: Into<alloy_rpc_types_eth::Log>,
    {
        match eth_filter.filter_changes(id.clone()).await.unwrap() {
            FilterChanges::Logs(logs) => {
                logs.into_iter().map(|log| log.into().block_number.unwrap()).collect()
            }
            FilterChanges::Empty => Vec::new(),
            changes => panic!("unexpected changes: {changes:?}"),
        }
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
    async fn test_range_block_mode_empty_range() {
        let provider = MockEthProvider::default();
        let eth_api = build_test_eth_api(provider);

        let eth_filter =
            super::EthFilter::new(eth_api, EthFilterConfig::default(), Runtime::test());
        let filter_inner = eth_filter.inner;

        let headers = vec![];
        let max_range = 100;

        let mut range_mode = RangeBlockMode {
            filter_inner,
            iter: headers.into_iter().peekable(),
            next: VecDeque::new(),
            max_range,
            parallel: false,

            pending_tasks: FuturesOrdered::new(),
        };

        let result = range_mode.next().await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_range_block_mode_queued_results_priority() {
        let provider = MockEthProvider::default();

        let headers = vec![
            SealedHeader::new(
                alloy_consensus::Header { number: 100, ..Default::default() },
                FixedBytes::random(),
            ),
            SealedHeader::new(
                alloy_consensus::Header { number: 101, ..Default::default() },
                FixedBytes::random(),
            ),
        ];
        for header in &headers {
            provider.add_header(header.hash(), header.header().clone());
            provider.add_receipts(header.number(), vec![]);
        }

        let eth_api = build_test_eth_api(provider);

        let eth_filter =
            super::EthFilter::new(eth_api, EthFilterConfig::default(), Runtime::test());
        let filter_inner = eth_filter.inner;

        // create specific mock results to test ordering
        let expected_block_hash_1 = FixedBytes::from([1u8; 32]);
        let expected_block_hash_2 = FixedBytes::from([2u8; 32]);

        // create mock receipts to test receipt handling
        let mock_receipt_1 =
            reth_ethereum_primitives::Receipt::standard(TxType::Legacy, true, 100_000, vec![]);
        let mock_receipt_2 =
            reth_ethereum_primitives::Receipt::standard(TxType::Eip1559, true, 200_000, vec![]);
        let mock_receipt_3 =
            reth_ethereum_primitives::Receipt::standard(TxType::Eip2930, false, 150_000, vec![]);

        let mock_result_1 = ReceiptBlockResult {
            receipts: Arc::new(vec![mock_receipt_1.clone(), mock_receipt_2.clone()]),
            recovered_block: None,
            header: SealedHeader::new(
                alloy_consensus::Header { number: 42, ..Default::default() },
                expected_block_hash_1,
            ),
        };

        let mock_result_2 = ReceiptBlockResult {
            receipts: Arc::new(vec![mock_receipt_3.clone()]),
            recovered_block: None,
            header: SealedHeader::new(
                alloy_consensus::Header { number: 43, ..Default::default() },
                expected_block_hash_2,
            ),
        };

        let mut range_mode = RangeBlockMode {
            filter_inner,
            iter: headers.into_iter().peekable(),
            next: VecDeque::from([mock_result_1, mock_result_2]), // Queue two results
            max_range: 100,
            parallel: false,

            pending_tasks: FuturesOrdered::new(),
        };

        // first call should return the first queued result (FIFO order)
        let result1 = range_mode.next().await;
        assert!(result1.is_ok());
        let receipt_result1 = result1.unwrap().unwrap();
        assert_eq!(receipt_result1.header.hash(), expected_block_hash_1);
        assert_eq!(receipt_result1.header.number, 42);

        // verify receipts
        assert_eq!(receipt_result1.receipts.len(), 2);
        assert_eq!(receipt_result1.receipts[0].tx_type(), mock_receipt_1.tx_type());
        assert_eq!(
            receipt_result1.receipts[0].cumulative_gas_used(),
            mock_receipt_1.cumulative_gas_used()
        );
        assert_eq!(receipt_result1.receipts[0].success(), mock_receipt_1.success());
        assert_eq!(receipt_result1.receipts[1].tx_type(), mock_receipt_2.tx_type());
        assert_eq!(
            receipt_result1.receipts[1].cumulative_gas_used(),
            mock_receipt_2.cumulative_gas_used()
        );
        assert_eq!(receipt_result1.receipts[1].success(), mock_receipt_2.success());

        // second call should return the second queued result
        let result2 = range_mode.next().await;
        assert!(result2.is_ok());
        let receipt_result2 = result2.unwrap().unwrap();
        assert_eq!(receipt_result2.header.hash(), expected_block_hash_2);
        assert_eq!(receipt_result2.header.number, 43);

        // verify receipts
        assert_eq!(receipt_result2.receipts.len(), 1);
        assert_eq!(receipt_result2.receipts[0].tx_type(), mock_receipt_3.tx_type());
        assert_eq!(
            receipt_result2.receipts[0].cumulative_gas_used(),
            mock_receipt_3.cumulative_gas_used()
        );
        assert_eq!(receipt_result2.receipts[0].success(), mock_receipt_3.success());

        // queue should now be empty
        assert!(range_mode.next.is_empty());

        let result3 = range_mode.next().await;
        assert!(result3.is_ok());
    }

    #[tokio::test]
    async fn test_range_block_mode_single_block_missing_receipts() {
        let provider = MockEthProvider::default();
        let eth_api = build_test_eth_api(provider);

        let eth_filter =
            super::EthFilter::new(eth_api, EthFilterConfig::default(), Runtime::test());
        let filter_inner = eth_filter.inner;

        let headers = vec![SealedHeader::new(
            alloy_consensus::Header { number: 100, ..Default::default() },
            FixedBytes::random(),
        )];

        let mut range_mode = RangeBlockMode {
            filter_inner,
            iter: headers.into_iter().peekable(),
            next: VecDeque::new(),
            max_range: 100,
            parallel: false,

            pending_tasks: FuturesOrdered::new(),
        };

        // a block whose header matched the filter but whose receipts are gone must not be
        // silently skipped
        let Err(err) = range_mode.next().await else { panic!("missing receipts must be an error") };
        assert!(matches!(err, EthFilterError::ReceiptsUnavailable(100)), "{err:?}");
    }

    #[tokio::test]
    async fn test_range_block_mode_provider_receipts() {
        let provider = MockEthProvider::default();

        let header_1 = alloy_consensus::Header { number: 100, ..Default::default() };
        let header_2 = alloy_consensus::Header { number: 101, ..Default::default() };
        let header_3 = alloy_consensus::Header { number: 102, ..Default::default() };

        let block_hash_1 = FixedBytes::random();
        let block_hash_2 = FixedBytes::random();
        let block_hash_3 = FixedBytes::random();

        provider.add_header(block_hash_1, header_1.clone());
        provider.add_header(block_hash_2, header_2.clone());
        provider.add_header(block_hash_3, header_3.clone());

        // create mock receipts to test provider fetching with mock logs
        let mock_log = alloy_primitives::Log {
            address: alloy_primitives::Address::ZERO,
            data: alloy_primitives::LogData::new_unchecked(vec![], alloy_primitives::Bytes::new()),
        };

        let receipt_100_1 = reth_ethereum_primitives::Receipt::standard(
            TxType::Legacy,
            true,
            21_000,
            vec![mock_log.clone()],
        );
        let receipt_100_2 = reth_ethereum_primitives::Receipt::standard(
            TxType::Eip1559,
            true,
            42_000,
            vec![mock_log.clone()],
        );
        let receipt_101_1 = reth_ethereum_primitives::Receipt::standard(
            TxType::Eip2930,
            false,
            30_000,
            vec![mock_log.clone()],
        );

        provider.add_receipts(100, vec![receipt_100_1.clone(), receipt_100_2.clone()]);
        provider.add_receipts(101, vec![receipt_101_1.clone()]);
        // a block without transactions, which a provider reports as an empty list
        provider.add_receipts(102, vec![]);

        let eth_api = build_test_eth_api(provider);

        let eth_filter =
            super::EthFilter::new(eth_api, EthFilterConfig::default(), Runtime::test());
        let filter_inner = eth_filter.inner;

        let headers = vec![
            SealedHeader::new(header_1, block_hash_1),
            SealedHeader::new(header_2, block_hash_2),
            SealedHeader::new(header_3, block_hash_3),
        ];

        let mut range_mode = RangeBlockMode {
            filter_inner,
            iter: headers.into_iter().peekable(),
            next: VecDeque::new(),
            max_range: 3, // include the 3 blocks in the first queried results
            parallel: false,

            pending_tasks: FuturesOrdered::new(),
        };

        // first call should fetch receipts from provider and return first block with receipts
        let result = range_mode.next().await;
        assert!(result.is_ok());
        let receipt_result = result.unwrap().unwrap();

        assert_eq!(receipt_result.header.hash(), block_hash_1);
        assert_eq!(receipt_result.header.number, 100);
        assert_eq!(receipt_result.receipts.len(), 2);

        // verify receipts
        assert_eq!(receipt_result.receipts[0].tx_type(), receipt_100_1.tx_type());
        assert_eq!(
            receipt_result.receipts[0].cumulative_gas_used(),
            receipt_100_1.cumulative_gas_used()
        );
        assert_eq!(receipt_result.receipts[0].success(), receipt_100_1.success());

        assert_eq!(receipt_result.receipts[1].tx_type(), receipt_100_2.tx_type());
        assert_eq!(
            receipt_result.receipts[1].cumulative_gas_used(),
            receipt_100_2.cumulative_gas_used()
        );
        assert_eq!(receipt_result.receipts[1].success(), receipt_100_2.success());

        // second call should return the second block with receipts
        let result2 = range_mode.next().await;
        assert!(result2.is_ok());
        let receipt_result2 = result2.unwrap().unwrap();

        assert_eq!(receipt_result2.header.hash(), block_hash_2);
        assert_eq!(receipt_result2.header.number, 101);
        assert_eq!(receipt_result2.receipts.len(), 1);

        // verify receipts
        assert_eq!(receipt_result2.receipts[0].tx_type(), receipt_101_1.tx_type());
        assert_eq!(
            receipt_result2.receipts[0].cumulative_gas_used(),
            receipt_101_1.cumulative_gas_used()
        );
        assert_eq!(receipt_result2.receipts[0].success(), receipt_101_1.success());

        // third call should return None since no more blocks with receipts
        let result3 = range_mode.next().await;
        assert!(result3.is_ok());
        assert!(result3.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_range_block_mode_iterator_exhaustion() {
        let provider = MockEthProvider::default();

        let header_100 = alloy_consensus::Header { number: 100, ..Default::default() };
        let header_101 = alloy_consensus::Header { number: 101, ..Default::default() };

        let block_hash_100 = FixedBytes::random();
        let block_hash_101 = FixedBytes::random();

        // Associate headers with hashes first
        provider.add_header(block_hash_100, header_100.clone());
        provider.add_header(block_hash_101, header_101.clone());

        // Add mock receipts so headers are actually processed
        let mock_receipt =
            reth_ethereum_primitives::Receipt::standard(TxType::Legacy, true, 21_000, vec![]);
        provider.add_receipts(100, vec![mock_receipt.clone()]);
        provider.add_receipts(101, vec![mock_receipt.clone()]);

        let eth_api = build_test_eth_api(provider);

        let eth_filter =
            super::EthFilter::new(eth_api, EthFilterConfig::default(), Runtime::test());
        let filter_inner = eth_filter.inner;

        let headers = vec![
            SealedHeader::new(header_100, block_hash_100),
            SealedHeader::new(header_101, block_hash_101),
        ];

        let mut range_mode = RangeBlockMode {
            filter_inner,
            iter: headers.into_iter().peekable(),
            next: VecDeque::new(),
            max_range: 1,
            parallel: false,

            pending_tasks: FuturesOrdered::new(),
        };

        let result1 = range_mode.next().await;
        assert!(result1.is_ok());
        assert!(result1.unwrap().is_some()); // Should have processed block 100

        assert!(range_mode.iter.peek().is_some()); // Should still have block 101

        let result2 = range_mode.next().await;
        assert!(result2.is_ok());
        assert!(result2.unwrap().is_some()); // Should have processed block 101

        // now iterator should be exhausted
        assert!(range_mode.iter.peek().is_none());

        // further calls should return None
        let result3 = range_mode.next().await;
        assert!(result3.is_ok());
        assert!(result3.unwrap().is_none());
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

        let mock_receipt = reth_ethereum_primitives::Receipt::standard(
            TxType::Legacy,
            true,
            21_000,
            vec![mock_log],
        );

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
        assert_eq!(receipt_block_result.receipts[0].tx_type(), mock_receipt.tx_type());
        assert_eq!(
            receipt_block_result.receipts[0].cumulative_gas_used(),
            mock_receipt.cumulative_gas_used()
        );
        assert_eq!(receipt_block_result.receipts[0].success(), mock_receipt.success());

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

    /// Returns a provider with blocks 100 to 102, each with one log with one topic and `data`.
    fn provider_with_one_log_per_block(data: alloy_primitives::Bytes) -> MockEthProvider {
        let provider = MockEthProvider::default();

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
            data: alloy_primitives::LogData::new_unchecked(vec![FixedBytes::ZERO], data),
        };

        let receipt = reth_ethereum_primitives::Receipt::standard(
            TxType::Legacy,
            true,
            21_000,
            vec![mock_log],
        );

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

        provider
    }

    #[tokio::test]
    async fn test_log_limit_retry_range_excludes_overflow_block() {
        let provider = provider_with_one_log_per_block(alloy_primitives::Bytes::new());
        let eth_api = build_test_eth_api(provider);
        let eth_filter = EthFilter::new(eth_api, EthFilterConfig::default(), Runtime::test());
        let err = eth_filter
            .inner
            .clone()
            .get_logs_in_block_range(
                Filter::default(),
                100,
                102,
                QueryLimits { max_logs_per_response: Some(2), ..Default::default() },
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
    async fn test_response_size_retry_range_excludes_overflow_block() {
        // Each log is at least 300 + 2 * 1_032 and at most 436 + 2 * 1_032 bytes.
        let provider = provider_with_one_log_per_block(vec![0u8; 1_000].into());
        let eth_api = build_test_eth_api(provider);
        let eth_filter = EthFilter::new(eth_api, EthFilterConfig::default(), Runtime::test());
        let transport_accepts = |logs, max_response_size| {
            MethodResponse::response(
                Id::Number(1),
                ResponsePayload::success(logs),
                max_response_size,
            )
            .is_success()
        };

        // The limit, the end of the retry range and whether the transport accepts that range:
        // three blocks exceed 7_000, two blocks may exceed 5_000 and 4_800, and the first block
        // alone exceeds 2_000, which is left to the transport like any single block.
        for (max_response_size, retry_to_block, accepted) in
            [(7_000, 101, true), (5_000, 100, true), (4_800, 100, true), (2_000, 100, false)]
        {
            let limits =
                QueryLimits { max_response_size: Some(max_response_size), ..Default::default() };
            let err = eth_filter
                .inner
                .clone()
                .get_logs_in_block_range(Filter::default(), 100, 102, limits)
                .await
                .expect_err("range should exceed max response size");
            let EthFilterError::QueryExceedsMaxResponseSize { max_bytes, from_block, to_block } =
                err
            else {
                panic!("unexpected error: {err:?}");
            };
            assert_eq!((max_bytes, from_block, to_block), (max_response_size, 100, retry_to_block));

            let logs = eth_filter
                .inner
                .clone()
                .get_logs_in_block_range(Filter::default(), 100, retry_to_block, limits)
                .await
                .unwrap();
            assert_eq!(logs.len(), (100..=retry_to_block).count());
            assert_eq!(transport_accepts(logs, max_response_size), accepted);
        }

        // The lower bound alone admits two blocks at 4_800, which the transport rejects.
        let limits = QueryLimits { max_response_size: Some(4_800), ..Default::default() };
        let logs = eth_filter
            .inner
            .clone()
            .get_logs_in_block_range(Filter::default(), 100, 101, limits)
            .await
            .unwrap();
        assert!(!transport_accepts(logs, 4_800));
    }

    #[test]
    fn test_log_json_size_bounds() {
        let rpc_log = |topics, data_len, quantity, block_timestamp| alloy_rpc_types_eth::Log {
            inner: alloy_primitives::Log {
                address: alloy_primitives::Address::ZERO,
                data: alloy_primitives::LogData::new_unchecked(
                    vec![FixedBytes::ZERO; topics],
                    vec![0u8; data_len].into(),
                ),
            },
            block_hash: Some(FixedBytes::ZERO),
            block_number: Some(quantity),
            block_timestamp,
            transaction_hash: Some(FixedBytes::ZERO),
            transaction_index: Some(quantity),
            log_index: Some(quantity),
            removed: false,
        };
        // The smallest and the widest encoding of a log.
        let smallest = rpc_log(0, 0, 0, None);
        let widest = rpc_log(4, 1_000, u64::MAX, Some(u64::MAX));
        let hex_len = |log: &alloy_rpc_types_eth::Log| {
            (size_of_val(log.topics()) + log.data().data.len()) * 2
        };

        for log in [&smallest, &widest] {
            let (min, max) = (MIN_LOG_JSON_SIZE + hex_len(log), MAX_LOG_JSON_SIZE + hex_len(log));
            let json_size = serde_json::to_vec(log).unwrap().len();
            // The upper bound includes the separator to the next log.
            assert!(min <= json_size && json_size < max, "{min} <= {json_size} < {max}");
        }

        // The envelope allowance covers the widest numeric request id.
        let max = MAX_RESPONSE_ENVELOPE_SIZE + MAX_LOG_JSON_SIZE + hex_len(&widest);
        let response = MethodResponse::response(
            Id::Number(u64::MAX),
            ResponsePayload::success(vec![widest]),
            usize::MAX,
        );
        assert!(response.into_json().len() <= max);
    }

    #[tokio::test]
    async fn test_non_consecutive_headers_after_bloom_filter() {
        let provider = MockEthProvider::default();

        // Create 4 headers where only blocks 100 and 102 will match bloom filter
        let mut expected_hashes = vec![];
        let mut prev_hash = alloy_primitives::B256::default();

        // Create a transaction for blocks that will have receipts
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

        let receipt = reth_ethereum_primitives::Receipt::standard(
            TxType::Legacy,
            true,
            21_000,
            vec![mock_log],
        );

        provider.add_receipts(100, vec![receipt.clone()]);
        provider.add_receipts(101, vec![]);
        provider.add_receipts(102, vec![receipt.clone()]);
        provider.add_receipts(103, vec![]);

        // Add block body indices for each block so receipts can be fetched
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
        let receipt = Receipt::standard(
            TxType::Legacy,
            true,
            21_000,
            vec![Log {
                address: Address::ZERO,
                data: LogData::new_unchecked(vec![], Bytes::new()),
            }],
        );

        // an empty filter matches every header, so every window is fetched in parallel while
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

        // the log limit ends the scan in the window that exceeds it and names the last block that
        // fit as the range to retry with
        let err = eth_filter
            .inner
            .clone()
            .get_logs_in_block_range(
                Filter::default(),
                0,
                2_500,
                QueryLimits { max_logs_per_response: Some(1), ..Default::default() },
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

    /// How long a test waits for the feed to deliver or stop.
    const TIMEOUT: Duration = Duration::from_secs(5);

    /// Returns a provider that has a best block, which installing a filter reads.
    fn provider_with_head() -> MockEthProvider {
        let provider = MockEthProvider::default();
        provider.add_header(FixedBytes::random(), alloy_consensus::Header::default());
        provider
    }

    fn hash_feed_sender<Eth: EthApiTypes>(
        filter: &EthFilter<Eth>,
    ) -> Option<broadcast::Sender<TxHash>> {
        filter.inner.pending_transaction_hashes.sender.lock().as_ref()?.upgrade()
    }

    /// Returns the kind of the pending transaction filter with the given id.
    async fn pending_kind<Eth: EthApiTypes + 'static>(
        filter: &EthFilter<Eth>,
        id: &FilterId,
    ) -> PendingTransactionKind<RpcTransaction<Eth::NetworkTypes>> {
        match filter.active_filters().inner.lock().await.get(id).unwrap().kind.clone() {
            FilterKind::PendingTransaction(kind) => kind,
            kind => panic!("not a pending transaction filter: {kind:?}"),
        }
    }

    /// Drains the hash filter until it returned `count` hashes.
    async fn drain_hashes<Eth: EthApiTypes + 'static>(
        filter: &EthFilter<Eth>,
        id: &FilterId,
        count: usize,
    ) -> Vec<TxHash> {
        let kind = pending_kind(filter, id).await;
        tokio::time::timeout(TIMEOUT, async {
            let mut hashes = Vec::new();
            while hashes.len() < count {
                match kind.drain().await {
                    FilterChanges::Hashes(new) => hashes.extend(new),
                    changes => panic!("unexpected changes {changes:?}"),
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            hashes
        })
        .await
        .expect("filter did not receive the transactions")
    }

    /// Waits until the hash feed has stopped.
    async fn wait_for_feed_stop<Eth: EthApiTypes>(filter: &EthFilter<Eth>) {
        tokio::time::timeout(TIMEOUT, async {
            while hash_feed_sender(filter).is_some() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("feed did not stop")
    }

    #[tokio::test]
    async fn pending_hash_filters_share_a_feed() {
        let eth_api = build_test_eth_api(provider_with_head());
        let pool = eth_api.pool().clone();
        let eth_filter = EthFilter::new(eth_api, EthFilterConfig::default(), Runtime::test());

        assert!(hash_feed_sender(&eth_filter).is_none());
        let first = eth_filter.new_pending_transaction_filter(None).await.unwrap();
        let second = eth_filter.new_pending_transaction_filter(None).await.unwrap();
        assert_eq!(hash_feed_sender(&eth_filter).unwrap().receiver_count(), 2);

        let hash = pool
            .add_transaction(TransactionOrigin::External, MockTransaction::eip1559())
            .await
            .unwrap()
            .hash;
        assert_eq!(drain_hashes(&eth_filter, &first, 1).await, [hash]);
        assert_eq!(drain_hashes(&eth_filter, &second, 1).await, [hash]);

        // A filter only sees the transactions that arrive after it was created.
        let third = eth_filter.new_pending_transaction_filter(None).await.unwrap();
        let next_hash = pool
            .add_transaction(TransactionOrigin::External, MockTransaction::eip1559())
            .await
            .unwrap()
            .hash;
        for id in [&first, &second, &third] {
            assert_eq!(drain_hashes(&eth_filter, id, 1).await, [next_hash]);
        }
    }

    #[tokio::test]
    async fn pending_hash_feed_stops_with_its_last_filter() {
        let eth_api = build_test_eth_api(provider_with_head());
        let pool = eth_api.pool().clone();
        let eth_filter = EthFilter::new(eth_api, EthFilterConfig::default(), Runtime::test());

        let first = eth_filter.new_pending_transaction_filter(None).await.unwrap();
        let second = eth_filter.new_pending_transaction_filter(None).await.unwrap();

        // The feed outlives a filter that is uninstalled.
        assert!(eth_filter.uninstall_filter(first).await.unwrap());
        assert_eq!(hash_feed_sender(&eth_filter).unwrap().receiver_count(), 1);

        // It stops once the last filter is evicted as stale.
        eth_filter.clear_stale_filters(Instant::now() + Duration::from_secs(3600)).await;
        assert!(!eth_filter.active_filters().contains(&second).await);
        wait_for_feed_stop(&eth_filter).await;

        // A later filter starts a new producer.
        let third = eth_filter.new_pending_transaction_filter(None).await.unwrap();
        let hash = pool
            .add_transaction(TransactionOrigin::External, MockTransaction::eip1559())
            .await
            .unwrap()
            .hash;
        assert_eq!(drain_hashes(&eth_filter, &third, 1).await, [hash]);
    }

    #[tokio::test]
    async fn lagging_hash_filter_keeps_the_newest_hashes() {
        const CAPACITY: usize = 4;

        let hashes = (0..10u8).map(|byte| TxHash::repeat_byte(byte + 1)).collect::<Vec<_>>();
        let feed = Arc::new(TxFeed::new());
        let runtime = Runtime::test();
        // The source ends after it is forwarded, which stops the producer, and the filter has
        // not been polled until then.
        let receiver = PendingTransactionsReceiver::new(
            feed.subscribe(&runtime, || (stream::iter(hashes.clone()), CAPACITY)),
        );
        tokio::time::timeout(TIMEOUT, async {
            while feed.sender.lock().is_some() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("feed did not stop");

        let FilterChanges::Hashes(drained) = receiver.drain::<()>().await else {
            panic!("expected hashes")
        };
        assert_eq!(drained, hashes[hashes.len() - CAPACITY..]);
        assert!(
            matches!(receiver.drain::<()>().await, FilterChanges::Hashes(hashes) if hashes.is_empty())
        );
    }

    #[tokio::test]
    async fn hash_feed_holds_as_many_hashes_as_the_pool_listener() {
        // More than the default listener buffer of 2048, which a fixed-size feed would cap at.
        const BUFFER: usize = 4096;
        const TXS: usize = 3000;

        let pool = TestPool::from(TestPoolBuilder::default().with_config(PoolConfig {
            pending_tx_listener_buffer_size: BUFFER,
            ..Default::default()
        }));
        let eth_api = build_test_eth_api_with_pool(provider_with_head(), pool.clone());
        let eth_filter = EthFilter::new(eth_api, EthFilterConfig::default(), Runtime::test());
        let id = eth_filter.new_pending_transaction_filter(None).await.unwrap();

        let txs = (0..TXS).map(|_| MockTransaction::eip1559()).collect::<Vec<_>>();
        let hashes = txs.iter().map(|tx| *tx.hash()).collect::<Vec<_>>();
        for result in pool.add_transactions(TransactionOrigin::External, txs).await {
            result.unwrap();
        }

        // Poll only once the feed holds every hash, so the filter has to keep all of them.
        let sender = hash_feed_sender(&eth_filter).unwrap();
        tokio::time::timeout(TIMEOUT, async {
            while sender.len() < TXS {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("feed did not hold every hash");
        assert_eq!(drain_hashes(&eth_filter, &id, TXS).await, hashes);
    }

    #[tokio::test]
    async fn feeds_cap_a_huge_pool_listener_buffer() {
        // The pool's listener channels never allocate their buffer, but a feed of this capacity
        // could not be allocated.
        const BUFFER: usize = usize::MAX >> 4;

        let pool = TestPool::from(TestPoolBuilder::default().with_config(PoolConfig {
            pending_tx_listener_buffer_size: BUFFER,
            new_tx_listener_buffer_size: BUFFER,
            ..Default::default()
        }));
        let eth_api = build_test_eth_api_with_pool(provider_with_head(), pool.clone());
        let eth_filter = EthFilter::new(eth_api, EthFilterConfig::default(), Runtime::test());

        let id = eth_filter.new_pending_transaction_filter(None).await.unwrap();
        eth_filter
            .new_pending_transaction_filter(Some(PendingTransactionFilterKind::Full))
            .await
            .unwrap();
        let hash = pool
            .add_transaction(TransactionOrigin::External, MockTransaction::eip1559())
            .await
            .unwrap()
            .hash;
        assert_eq!(drain_hashes(&eth_filter, &id, 1).await, [hash]);
    }

    #[tokio::test]
    async fn full_pending_filters_share_a_feed() {
        let eth_api = build_test_eth_api(provider_with_head());
        let pool = eth_api.pool().clone();
        let eth_filter = EthFilter::new(eth_api, EthFilterConfig::default(), Runtime::test());

        let first = eth_filter
            .new_pending_transaction_filter(Some(PendingTransactionFilterKind::Full))
            .await
            .unwrap();
        let second = eth_filter
            .new_pending_transaction_filter(Some(PendingTransactionFilterKind::Full))
            .await
            .unwrap();

        let hash = pool
            .add_transaction(TransactionOrigin::External, MockTransaction::eip1559())
            .await
            .unwrap()
            .hash;

        for id in [&first, &second] {
            let kind = pending_kind(&eth_filter, id).await;
            let txs = tokio::time::timeout(TIMEOUT, async {
                loop {
                    match kind.drain().await {
                        FilterChanges::Transactions(txs) if !txs.is_empty() => break txs,
                        FilterChanges::Transactions(_) => {
                            tokio::time::sleep(Duration::from_millis(5)).await
                        }
                        changes => panic!("unexpected changes {changes:?}"),
                    }
                }
            })
            .await
            .expect("filter did not receive the transaction");
            assert_eq!(txs.iter().map(|tx| tx.tx_hash()).collect::<Vec<_>>(), [hash]);
        }
    }

    #[tokio::test]
    async fn full_pending_filter_is_served_through_filter_changes() {
        let eth_api = build_test_eth_api(provider_with_head());
        let pool = eth_api.pool().clone();
        let eth_filter = EthFilter::new(eth_api, EthFilterConfig::default(), Runtime::test());

        let id = eth_filter
            .new_pending_transaction_filter(Some(PendingTransactionFilterKind::Full))
            .await
            .unwrap();
        let hash = pool
            .add_transaction(TransactionOrigin::External, MockTransaction::eip1559())
            .await
            .unwrap()
            .hash;

        // The feed forwards the transaction asynchronously, so poll until it arrives.
        let txs = tokio::time::timeout(TIMEOUT, async {
            loop {
                match eth_filter.filter_changes(id.clone()).await.unwrap() {
                    FilterChanges::Transactions(txs) if !txs.is_empty() => break txs,
                    FilterChanges::Transactions(_) => {
                        tokio::time::sleep(Duration::from_millis(5)).await
                    }
                    changes => panic!("unexpected changes {changes:?}"),
                }
            }
        })
        .await
        .expect("filter did not receive the transaction");
        assert_eq!(txs.iter().map(|tx| tx.tx_hash()).collect::<Vec<_>>(), [hash]);
    }

    #[tokio::test]
    async fn too_many_filters_are_rejected() {
        let eth_api = build_test_eth_api(provider_with_head());
        let config = EthFilterConfig::default().max_active_filters(Some(2));
        let eth_filter = EthFilter::new(eth_api, config, Runtime::test());

        let block_filter = eth_filter.new_block_filter().await.unwrap();
        eth_filter.new_filter(Filter::new()).await.unwrap();

        for kind in [None, Some(PendingTransactionFilterKind::Full)] {
            let err = eth_filter.new_pending_transaction_filter(kind).await.unwrap_err();
            assert_eq!(err.code(), LIMIT_EXCEEDED_CODE);
            assert_eq!(err.message(), "too many active filters, max 2");
        }
        assert_eq!(eth_filter.new_block_filter().await.unwrap_err().code(), LIMIT_EXCEEDED_CODE);
        assert_eq!(
            eth_filter.new_filter(Filter::new()).await.unwrap_err().code(),
            LIMIT_EXCEEDED_CODE
        );
        assert_eq!(eth_filter.active_filters().len().await, 2);

        // A rejected pending filter must not start a feed.
        assert!(hash_feed_sender(&eth_filter).is_none());
        assert!(eth_filter.inner.full_pending_transactions.get().is_none());

        // Uninstalling frees a slot for any kind.
        assert!(eth_filter.uninstall_filter(block_filter).await.unwrap());
        eth_filter.new_pending_transaction_filter(None).await.unwrap();
        assert!(hash_feed_sender(&eth_filter).is_some());
        assert_eq!(
            eth_filter.new_block_filter().await.unwrap_err().message(),
            "too many active filters, max 2"
        );
    }

    #[tokio::test]
    async fn unlimited_filters_when_no_maximum_is_configured() {
        let eth_api = build_test_eth_api(provider_with_head());
        let config = EthFilterConfig::default().max_active_filters(None);
        let eth_filter = EthFilter::new(eth_api, config, Runtime::test());

        for _ in 0..50 {
            eth_filter.new_block_filter().await.unwrap();
        }
        assert_eq!(eth_filter.active_filters().len().await, 50);
    }

    /// Adds a block with logs in two of its five receipts to the provider, but not to the cache.
    fn add_block_with_logs(
        provider: &MockEthProvider,
    ) -> (SealedHeader<alloy_consensus::Header>, Vec<Receipt>) {
        let transactions = (0..5)
            .map(|nonce| {
                TransactionSigned::new_unhashed(
                    TxLegacy {
                        chain_id: Some(1),
                        nonce,
                        gas_price: 21_000,
                        gas_limit: 21_000,
                        ..Default::default()
                    }
                    .into(),
                    Signature::test_signature(),
                )
            })
            .collect();
        let log = |address: u8, topic: u8| Log {
            address: Address::repeat_byte(address),
            data: LogData::new_unchecked(vec![B256::repeat_byte(topic)], Bytes::new()),
        };
        let receipts = vec![
            Receipt::standard(TxType::Legacy, true, 21_000, vec![log(1, 1), log(2, 2)]),
            Receipt::standard(TxType::Legacy, true, 42_000, vec![]),
            Receipt::standard(TxType::Legacy, true, 63_000, vec![log(1, 2), log(3, 1), log(1, 1)]),
            Receipt::standard(TxType::Legacy, true, 84_000, vec![]),
            Receipt::standard(TxType::Legacy, true, 105_000, vec![]),
        ];

        let mut logs_bloom = Bloom::default();
        for log in receipts.iter().flat_map(|receipt| receipt.logs()) {
            logs_bloom.accrue_log(log);
        }
        let header = alloy_consensus::Header {
            number: 7,
            timestamp: 1_234,
            logs_bloom,
            ..Default::default()
        };
        let hash = header.hash_slow();
        provider.add_block(
            hash,
            Block {
                header: header.clone(),
                body: BlockBody { transactions, ..Default::default() },
            },
        );
        provider.add_receipts(7, receipts.clone());
        provider.add_block_body_indices(7, StoredBlockBodyIndices { first_tx_num: 0, tx_count: 5 });

        (SealedHeader::new(header, hash), receipts)
    }

    #[tokio::test]
    async fn test_logs_at_block_hash_from_provider() {
        let provider = MockEthProvider::default();
        let (header, receipts) = add_block_with_logs(&provider);
        let eth_api = build_test_eth_api(provider.clone());
        let eth_filter =
            EthFilter::new(eth_api.clone(), EthFilterConfig::default(), Runtime::test());

        let logs = eth_filter
            .logs_for_filter(Filter::new().at_block_hash(header.hash()), QueryLimits::default())
            .await
            .unwrap();
        let tx_hashes = provider
            .transactions_by_block(header.hash().into())
            .unwrap()
            .unwrap()
            .iter()
            .map(|tx| *tx.tx_hash())
            .collect::<Vec<_>>();
        assert_eq!(
            logs.iter()
                .map(|log| (
                    log.transaction_hash.unwrap(),
                    log.transaction_index.unwrap(),
                    log.log_index.unwrap()
                ))
                .collect::<Vec<_>>(),
            [
                (tx_hashes[0], 0, 0),
                (tx_hashes[0], 0, 1),
                (tx_hashes[2], 2, 2),
                (tx_hashes[2], 2, 3),
                (tx_hashes[2], 2, 4),
            ]
        );
        assert_eq!(
            logs.iter().map(|log| log.inner.clone()).collect::<Vec<_>>(),
            receipts.iter().flat_map(|receipt| receipt.logs().iter().cloned()).collect::<Vec<_>>()
        );
        for log in &logs {
            assert_eq!(log.block_hash, Some(header.hash()));
            assert_eq!(log.block_number, Some(7));
            assert_eq!(log.block_timestamp, Some(1_234));
            assert!(!log.removed);
        }

        for filter in [
            Filter::new(),
            Filter::new().address(Address::repeat_byte(1)),
            Filter::new().event_signature(B256::repeat_byte(1)),
            Filter::new().address(Address::repeat_byte(1)).event_signature(B256::repeat_byte(2)),
            Filter::new().address(Address::repeat_byte(3)).event_signature(B256::repeat_byte(2)),
            Filter::new().address(Address::repeat_byte(4)),
        ] {
            let filter = filter.at_block_hash(header.hash());
            let logs =
                eth_filter.logs_for_filter(filter.clone(), QueryLimits::default()).await.unwrap();

            // the logs looking up each transaction by its id produces.
            let mut expected = Vec::new();
            append_matching_block_logs(
                &mut expected,
                eth_api.converter(),
                ProviderOrBlock::Provider(&provider),
                &filter,
                &header,
                &receipts,
                false,
            )
            .unwrap();
            assert_eq!(logs, expected, "{filter:?}");

            // `eth_getFilterLogs` serves an installed filter the same way.
            let id = EthFilterApiServer::new_filter(&eth_filter, filter).await.unwrap();
            assert_eq!(EthFilter::filter_logs(&eth_filter, id).await.unwrap(), expected);
        }
    }

    #[tokio::test]
    async fn test_logs_at_block_hash_rejects_replaced_block() {
        let provider = MockEthProvider::default();
        let (header, receipts) = add_block_with_logs(&provider);
        let eth_api = build_test_eth_api(provider);

        // a block at the same height whose hash no longer resolves, as after a reorg, must not be
        // served with the transactions found at that height, whether they are read for the whole
        // block or for the one receipt with a matching log.
        let replaced = SealedHeader::new(header.clone_header(), B256::repeat_byte(0xab));
        // a block at a height that no longer exists makes the lookups fail, which is reported the
        // same way.
        let dropped = SealedHeader::new(
            alloy_consensus::Header { number: 8, ..header.clone_header() },
            B256::repeat_byte(0xab),
        );
        for (replaced, filter) in [
            (&replaced, Filter::new()),
            (&replaced, Filter::new().address(Address::repeat_byte(3))),
            (&dropped, Filter::new().address(Address::repeat_byte(3))),
        ] {
            let err = EthFilterInner::block_logs_from_provider(
                &eth_api,
                &filter.at_block_hash(replaced.hash()),
                replaced,
                &receipts,
            )
            .unwrap_err();
            assert!(
                matches!(
                    &err,
                    EthFilterError::EthAPIError(EthApiError::HeaderNotFound(id))
                        if *id == BlockId::from(replaced.hash())
                ),
                "{err:?}"
            );
        }
    }

    #[tokio::test]
    async fn test_logs_at_block_hash_caches_receipts() {
        let provider = MockEthProvider::default();
        let (header, _) = add_block_with_logs(&provider);
        let eth_api = build_test_eth_api(provider);
        let eth_filter =
            EthFilter::new(eth_api.clone(), EthFilterConfig::default(), Runtime::test());
        let cached_receipts = async || {
            eth_api.cache().maybe_cached_block_and_receipts(header.hash()).await.unwrap().1
        };

        // a filter the bloom rules out leaves the receipts unread.
        let filter = Filter::new().address(Address::repeat_byte(9)).at_block_hash(header.hash());
        let logs = eth_filter.logs_for_filter(filter, QueryLimits::default()).await.unwrap();
        assert!(logs.is_empty());
        assert!(cached_receipts().await.is_none());

        // a matching filter reads them through the cache, which keeps them for later queries.
        let filter = Filter::new().at_block_hash(header.hash());
        let logs = eth_filter.logs_for_filter(filter, QueryLimits::default()).await.unwrap();
        assert_eq!(logs.len(), 5);
        assert!(cached_receipts().await.is_some());

        // with the receipts cached, the bloom still rules out a filter.
        let filter = Filter::new().address(Address::repeat_byte(9)).at_block_hash(header.hash());
        let logs = eth_filter.logs_for_filter(filter, QueryLimits::default()).await.unwrap();
        assert!(logs.is_empty());
    }

    #[tokio::test]
    async fn test_logs_at_block_hash_cached_block() {
        let provider = MockEthProvider::default();
        let (header, _) = add_block_with_logs(&provider);
        let eth_api = build_test_eth_api(provider);
        let eth_filter =
            EthFilter::new(eth_api.clone(), EthFilterConfig::default(), Runtime::test());
        let filter = Filter::new().at_block_hash(header.hash());

        let uncached =
            eth_filter.logs_for_filter(filter.clone(), QueryLimits::default()).await.unwrap();
        eth_api.cache().get_recovered_block(header.hash()).await.unwrap().unwrap();

        // a cached block is served without a blocking IO permit.
        let guard = eth_api.blocking_io_task_guard().clone();
        let _permits =
            guard.clone().acquire_many_owned(guard.available_permits() as u32).await.unwrap();
        let cached = tokio::time::timeout(
            Duration::from_secs(5),
            eth_filter.logs_for_filter(filter, QueryLimits::default()),
        )
        .await
        .expect("a cached block must not wait for a blocking IO permit")
        .unwrap();
        assert_eq!(cached, uncached);
    }

    #[tokio::test]
    async fn test_logs_at_block_hash_waits_for_blocking_io_permit() {
        let provider = MockEthProvider::default();
        let (header, _) = add_block_with_logs(&provider);
        let eth_api = build_test_eth_api(provider);

        // take every permit so the provider reads have to wait for one.
        let guard = eth_api.blocking_io_task_guard().clone();
        let permits =
            guard.clone().acquire_many_owned(guard.available_permits() as u32).await.unwrap();

        let eth_filter = EthFilter::new(eth_api, EthFilterConfig::default(), Runtime::test());
        let logs = eth_filter
            .logs_for_filter(Filter::new().at_block_hash(header.hash()), QueryLimits::default());
        tokio::pin!(logs);
        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut logs).await.is_err(),
            "provider reads must wait for a blocking IO permit"
        );

        drop(permits);
        assert_eq!(logs.await.unwrap().len(), 5);
    }

    #[tokio::test]
    async fn test_logs_at_block_hash_errors() {
        let provider = MockEthProvider::default();
        let header = alloy_consensus::Header { number: 3, ..Default::default() };
        let hash = header.hash_slow();
        // a block whose receipts were pruned.
        provider.add_block(hash, Block { header, body: Default::default() });
        let eth_filter = EthFilter::new(
            build_test_eth_api(provider),
            EthFilterConfig::default(),
            Runtime::test(),
        );

        let err = eth_filter
            .logs_for_filter(Filter::new().at_block_hash(hash), QueryLimits::default())
            .await
            .unwrap_err();
        assert!(matches!(err, EthFilterError::ReceiptsUnavailable(3)), "{err:?}");

        let unknown = B256::repeat_byte(0xab);
        let err = eth_filter
            .logs_for_filter(Filter::new().at_block_hash(unknown), QueryLimits::default())
            .await
            .unwrap_err();
        assert!(
            matches!(
                &err,
                EthFilterError::EthAPIError(EthApiError::HeaderNotFound(id))
                    if *id == BlockId::from(unknown)
            ),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn test_logs_at_block_hash_bloom_miss_skips_receipts() {
        let provider = MockEthProvider::default();
        let header = alloy_consensus::Header { number: 3, ..Default::default() };
        let hash = header.hash_slow();
        // a block whose receipts were pruned.
        provider.add_block(hash, Block { header, body: Default::default() });
        let eth_api = build_test_eth_api(provider);
        let eth_filter =
            EthFilter::new(eth_api.clone(), EthFilterConfig::default(), Runtime::test());

        for cached in [false, true] {
            if cached {
                eth_api.cache().get_recovered_block(hash).await.unwrap().unwrap();
            }

            // the empty bloom rules out the address, so the receipts are not needed.
            let logs = eth_filter
                .logs_for_filter(
                    Filter::new().address(Address::repeat_byte(1)).at_block_hash(hash),
                    QueryLimits::default(),
                )
                .await
                .unwrap();
            assert!(logs.is_empty(), "cached: {cached}");

            let err = eth_filter
                .logs_for_filter(Filter::new().at_block_hash(hash), QueryLimits::default())
                .await
                .unwrap_err();
            assert!(
                matches!(err, EthFilterError::ReceiptsUnavailable(3)),
                "cached: {cached}, {err:?}"
            );
        }
    }

    #[tokio::test]
    async fn test_logs_at_block_hash_cached_receipts() {
        let provider = MockEthProvider::default();
        let (header, _) = add_block_with_logs(&provider);
        let eth_api = build_test_eth_api(provider.clone());
        let eth_filter =
            EthFilter::new(eth_api.clone(), EthFilterConfig::default(), Runtime::test());
        let filter = Filter::new().at_block_hash(header.hash());

        let expected =
            eth_filter.logs_for_filter(filter.clone(), QueryLimits::default()).await.unwrap();
        assert_eq!(expected.len(), 5);

        // only the cache holds the receipts from here on, the block is not cached.
        eth_api.cache().get_receipts(header.hash()).await.unwrap().unwrap();
        provider.receipts.lock().remove(&header.number());

        let logs = eth_filter.logs_for_filter(filter, QueryLimits::default()).await.unwrap();
        assert_eq!(logs, expected);
    }

    #[tokio::test]
    async fn test_logs_at_block_hash_expired_history() {
        let provider = MockEthProvider::default();
        let (header, _) = add_block_with_logs(&provider);
        provider.set_earliest_block_number(header.number() + 1);
        let eth_api = build_test_eth_api(provider);
        let eth_filter =
            EthFilter::new(eth_api.clone(), EthFilterConfig::default(), Runtime::test());

        for cached in [false, true] {
            if cached {
                eth_api.cache().get_recovered_block(header.hash()).await.unwrap().unwrap();
            }

            // the bloom rules out the second filter, expired history is still reported first.
            for filter in [Filter::new(), Filter::new().address(Address::repeat_byte(4))] {
                let err = eth_filter
                    .logs_for_filter(filter.at_block_hash(header.hash()), QueryLimits::default())
                    .await
                    .unwrap_err();
                assert!(
                    matches!(
                        err,
                        EthFilterError::EthAPIError(EthApiError::PrunedHistoryUnavailable {
                            requested: 7,
                            earliest_available: 8
                        })
                    ),
                    "cached: {cached}, {err:?}"
                );
            }
        }
    }

    #[tokio::test]
    async fn test_logs_at_block_hash_reorged_out() {
        let provider = MockEthProvider::default();
        let (header, _) = add_block_with_logs(&provider);
        let eth_api = build_test_eth_api(provider.clone());
        let eth_filter =
            EthFilter::new(eth_api.clone(), EthFilterConfig::default(), Runtime::test());

        // the cache still holds the block after the provider dropped it.
        eth_api.cache().get_recovered_block(header.hash()).await.unwrap().unwrap();
        provider.headers.lock().remove(&header.hash());

        // a matching filter needs the receipts, which are gone with the block.
        let err = eth_filter
            .logs_for_filter(Filter::new().at_block_hash(header.hash()), QueryLimits::default())
            .await
            .unwrap_err();
        assert!(
            matches!(
                &err,
                EthFilterError::EthAPIError(EthApiError::HeaderNotFound(id))
                    if *id == BlockId::from(header.hash())
            ),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn test_filter_changes_only_delivers_new_logs() {
        let provider = MockEthProvider::default();
        add_blocks_with_log(&provider, 0..=3);
        let eth_filter = EthFilter::new(
            build_test_eth_api(provider.clone()),
            EthFilterConfig::default(),
            Runtime::test(),
        );

        // an explicit `fromBlock` bounds the filter but must not be rescanned on every poll
        let id = eth_filter.new_filter(Filter::new().from_block(0u64)).await.unwrap();

        assert_eq!(poll_log_blocks(&eth_filter, &id).await, vec![3]);
        assert_eq!(poll_log_blocks(&eth_filter, &id).await, Vec::<u64>::new());

        add_blocks_with_log(&provider, 4..=5);
        assert_eq!(poll_log_blocks(&eth_filter, &id).await, vec![4, 5]);
        assert_eq!(poll_log_blocks(&eth_filter, &id).await, Vec::<u64>::new());

        // `eth_getFilterLogs` still returns the filter's entire range
        assert_eq!(eth_filter.filter_logs(id).await.unwrap().len(), 6);
    }

    #[tokio::test]
    async fn test_filter_changes_clamps_future_to_block() {
        let provider = MockEthProvider::default();
        add_blocks_with_log(&provider, 0..=3);
        let eth_filter = EthFilter::new(
            build_test_eth_api(provider.clone()),
            EthFilterConfig::default(),
            Runtime::test(),
        );

        // a `toBlock` beyond the head bounds future polls instead of failing them
        let id = eth_filter.new_filter(Filter::new().to_block(100u64)).await.unwrap();
        assert_eq!(poll_log_blocks(&eth_filter, &id).await, vec![3]);

        add_blocks_with_log(&provider, 4..=5);
        assert_eq!(poll_log_blocks(&eth_filter, &id).await, vec![4, 5]);
    }

    #[tokio::test]
    async fn test_filter_changes_latest_tag_covers_all_new_blocks() {
        let provider = MockEthProvider::default();
        add_blocks_with_log(&provider, 0..=3);
        let eth_filter = EthFilter::new(
            build_test_eth_api(provider.clone()),
            EthFilterConfig::default(),
            Runtime::test(),
        );

        // `fromBlock: latest` means from the installation onwards, not only the head at poll time
        let id =
            eth_filter.new_filter(Filter::new().select(BlockNumberOrTag::Latest..)).await.unwrap();
        add_blocks_with_log(&provider, 4..=5);
        assert_eq!(poll_log_blocks(&eth_filter, &id).await, vec![3, 4, 5]);
    }

    #[tokio::test]
    async fn test_block_hash_filter_changes_delivers_once() {
        let provider = MockEthProvider::default();
        add_blocks_with_log(&provider, 0..=3);
        let eth_filter = EthFilter::new(
            build_test_eth_api(provider.clone()),
            EthFilterConfig::default(),
            Runtime::test(),
        );

        // a `blockHash` filter's block never changes, so it is delivered once and neither a
        // repeated poll nor a new head brings it back
        let hash = provider.header_by_number(1).unwrap().unwrap().hash_slow();
        let id = eth_filter.new_filter(Filter::new().at_block_hash(hash)).await.unwrap();
        assert_eq!(poll_log_blocks(&eth_filter, &id).await, vec![1]);
        assert_eq!(poll_log_blocks(&eth_filter, &id).await, Vec::<u64>::new());

        add_blocks_with_log(&provider, 4..=5);
        assert_eq!(poll_log_blocks(&eth_filter, &id).await, Vec::<u64>::new());

        // a block that is not canonical fails every poll instead of exhausting the filter
        let unknown = B256::repeat_byte(0xab);
        let id = eth_filter.new_filter(Filter::new().at_block_hash(unknown)).await.unwrap();
        for _ in 0..2 {
            let err = eth_filter.filter_changes(id.clone()).await.unwrap_err();
            assert!(
                matches!(
                    &err,
                    EthFilterError::EthAPIError(EthApiError::HeaderNotFound(id))
                        if *id == unknown.into()
                ),
                "{err:?}"
            );
        }
    }

    #[tokio::test]
    async fn test_filter_changes_pages_over_log_limit() {
        let provider = MockEthProvider::default();
        add_blocks_with_log(&provider, 0..=0);
        let eth_filter = EthFilter::new(
            build_test_eth_api(provider.clone()),
            EthFilterConfig::default().max_logs_per_response(1),
            Runtime::test(),
        );
        let id = eth_filter.new_filter(Filter::default()).await.unwrap();
        assert_eq!(poll_log_blocks(&eth_filter, &id).await, vec![0]);

        // three blocks with one log each exceed the cap, so each poll delivers the prefix that
        // fits and the next poll continues from there without losing or repeating a block
        add_blocks_with_log(&provider, 1..=3);
        assert_eq!(poll_log_blocks(&eth_filter, &id).await, vec![1]);
        assert_eq!(poll_log_blocks(&eth_filter, &id).await, vec![2]);
        assert_eq!(poll_log_blocks(&eth_filter, &id).await, vec![3]);
        assert_eq!(poll_log_blocks(&eth_filter, &id).await, Vec::<u64>::new());
    }

    #[tokio::test]
    async fn test_filter_changes_pages_over_response_size() {
        let provider = MockEthProvider::default();
        add_blocks_with_log(&provider, 0..=0);
        let eth_filter = EthFilter::new(
            build_test_eth_api(provider.clone()),
            EthFilterConfig::default().max_response_size(Some(5_000)),
            Runtime::test(),
        );
        let id = eth_filter.new_filter(Filter::default()).await.unwrap();
        assert_eq!(poll_log_blocks(&eth_filter, &id).await, vec![0]);

        // ten logs take at least 3_000 bytes, so two such blocks exceed the limit and each poll
        // delivers one block, without losing or repeating a block
        add_blocks_with_logs(&provider, 1..=3, 10);
        for block in 1..=3 {
            assert_eq!(poll_log_blocks(&eth_filter, &id).await, vec![block; 10]);
        }
        assert_eq!(poll_log_blocks(&eth_filter, &id).await, Vec::<u64>::new());
    }

    #[tokio::test]
    async fn test_filter_changes_fit_transport_limit() {
        let provider = MockEthProvider::default();
        add_blocks_with_log(&provider, 0..=0);
        let eth_filter = EthFilter::new(
            build_test_eth_api(provider.clone()),
            EthFilterConfig::default().max_response_size(Some(5_000)),
            Runtime::test(),
        );
        let id = eth_filter.new_filter(Filter::default()).await.unwrap();
        assert_eq!(poll_log_blocks(&eth_filter, &id).await, vec![0]);

        // two blocks of eight logs take at least 4_800 bytes, under the limit, but up to 7_104
        // bytes in the response, so each poll returns one block, which the transport accepts.
        add_blocks_with_logs(&provider, 1..=2, 8);
        for block in 1..=2 {
            let changes = eth_filter.filter_changes(id.clone()).await.unwrap();
            let FilterChanges::Logs(logs) = &changes else { panic!("{changes:?}") };
            assert_eq!(
                logs.iter().map(|log| log.block_number.unwrap()).collect::<Vec<_>>(),
                vec![block; 8]
            );
            let response =
                MethodResponse::response(Id::Number(1), ResponsePayload::success(changes), 5_000);
            assert!(response.is_success());
        }
        assert_eq!(poll_log_blocks(&eth_filter, &id).await, Vec::<u64>::new());
    }

    #[tokio::test]
    async fn test_failed_filter_poll_is_retried() {
        let provider = MockEthProvider::default();
        add_blocks_with_log(&provider, 0..=0);
        let eth_api = build_test_eth_api(provider.clone());
        let eth_filter =
            EthFilter::new(eth_api.clone(), EthFilterConfig::default(), Runtime::test());
        let id = eth_filter.new_filter(Filter::default()).await.unwrap();
        assert_eq!(poll_log_blocks(&eth_filter, &id).await, vec![0]);

        // a poll whose request is dropped before its scan completes does not consume the new
        // block.
        add_blocks_with_log(&provider, 1..=1);
        let guard = eth_api.blocking_io_task_guard().clone();
        let permits =
            guard.clone().acquire_many_owned(guard.available_permits() as u32).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(100), eth_filter.filter_changes(id.clone()))
                .await
                .is_err(),
            "the scan must wait for a blocking IO permit"
        );
        drop(permits);

        assert_eq!(poll_log_blocks(&eth_filter, &id).await, vec![1]);
    }

    #[tokio::test]
    async fn test_filter_poll_skips_pruned_receipts() {
        let provider = MockEthProvider::default();
        add_blocks_with_log(&provider, 0..=0);
        let eth_filter = EthFilter::new(
            build_test_eth_api(provider.clone()),
            EthFilterConfig::default(),
            Runtime::test(),
        );
        let id = eth_filter.new_filter(Filter::default()).await.unwrap();
        assert_eq!(poll_log_blocks(&eth_filter, &id).await, vec![0]);

        // the receipts of block 1 were pruned before the filter got to them.
        add_blocks_with_log(&provider, 1..=2);
        provider.receipts.lock().remove(&1);
        let err = eth_filter.filter_changes(id.clone()).await.unwrap_err();
        assert!(matches!(err, EthFilterError::ReceiptsUnavailable(1)), "{err:?}");

        // the failed poll's range is skipped instead of failing every later poll.
        add_blocks_with_log(&provider, 3..=3);
        assert_eq!(poll_log_blocks(&eth_filter, &id).await, vec![3]);
    }

    #[tokio::test]
    async fn test_filter_changes_delivers_oversized_block_whole() {
        let provider = MockEthProvider::default();
        add_blocks_with_log(&provider, 0..=0);
        let eth_filter = EthFilter::new(
            build_test_eth_api(provider.clone()),
            EthFilterConfig::default().max_logs_per_response(1),
            Runtime::test(),
        );
        let id = eth_filter.new_filter(Filter::default()).await.unwrap();
        assert_eq!(poll_log_blocks(&eth_filter, &id).await, vec![0]);

        // a block with more logs than the cap is delivered whole, as by `eth_getLogs`, instead of
        // stalling the filter on it, and the next poll moves on
        add_blocks_with_logs(&provider, 1..=1, 2);
        add_blocks_with_log(&provider, 2..=2);
        assert_eq!(poll_log_blocks(&eth_filter, &id).await, vec![1, 1]);
        assert_eq!(poll_log_blocks(&eth_filter, &id).await, vec![2]);
        assert_eq!(poll_log_blocks(&eth_filter, &id).await, Vec::<u64>::new());
    }

    #[tokio::test]
    async fn test_filter_changes_pages_over_block_limit() {
        let provider = MockEthProvider::default();
        add_blocks_with_log(&provider, 0..=0);
        let eth_filter = EthFilter::new(
            build_test_eth_api(provider.clone()),
            EthFilterConfig::default().max_blocks_per_filter(1),
            Runtime::test(),
        );
        let id = eth_filter.new_filter(Filter::default()).await.unwrap();

        // a backlog wider than the block limit is delivered over successive polls instead of
        // failing them all with `QueryExceedsMaxBlocks`; the limit bounds `to - from` as in
        // `eth_getLogs`, so a poll covers at most `max + 1` blocks
        add_blocks_with_log(&provider, 1..=3);
        assert_eq!(poll_log_blocks(&eth_filter, &id).await, vec![0, 1]);
        assert_eq!(poll_log_blocks(&eth_filter, &id).await, vec![2, 3]);
        assert_eq!(poll_log_blocks(&eth_filter, &id).await, Vec::<u64>::new());
    }

    #[tokio::test]
    async fn test_filter_changes_reports_an_invalid_range() {
        let provider = MockEthProvider::default();
        add_blocks_with_log(&provider, 0..=3);
        let eth_filter = EthFilter::new(
            build_test_eth_api(provider),
            EthFilterConfig::default(),
            Runtime::test(),
        );

        // a filter that can never match must not look like one the cursor exhausted
        let id =
            eth_filter.new_filter(Filter::new().from_block(3u64).to_block(1u64)).await.unwrap();
        let err = eth_filter.filter_changes(id).await.unwrap_err();
        assert!(matches!(err, EthFilterError::InvalidBlockRangeParams), "{err:?}");
    }

    #[tokio::test]
    async fn test_filter_changes_without_new_block() {
        let provider = MockEthProvider::default();
        provider.add_header(FixedBytes::random(), alloy_consensus::Header::default());
        let eth_filter = EthFilter::new(
            build_test_eth_api(provider),
            EthFilterConfig::default(),
            Runtime::test(),
        );
        let block_filter = eth_filter.new_block_filter().await.unwrap();
        let pending_filter = eth_filter.new_pending_transaction_filter(None).await.unwrap();

        // The first poll advances the filters past the head.
        eth_filter.filter_changes(block_filter.clone()).await.unwrap();
        eth_filter.filter_changes(pending_filter.clone()).await.unwrap();

        let tx = MockTransaction::eip1559();
        let hash = *tx.hash();
        eth_filter.pool().add_transaction(TransactionOrigin::External, tx).await.unwrap();

        // A block filter waits for the next block, a pending transaction filter does not.
        assert_eq!(eth_filter.filter_changes(block_filter).await.unwrap(), FilterChanges::Empty);
        // The feed forwards the transaction asynchronously, so poll until it arrives.
        let hashes = tokio::time::timeout(TIMEOUT, async {
            loop {
                match eth_filter.filter_changes(pending_filter.clone()).await.unwrap() {
                    FilterChanges::Hashes(hashes) if !hashes.is_empty() => break hashes,
                    FilterChanges::Hashes(_) => tokio::time::sleep(Duration::from_millis(5)).await,
                    changes => panic!("unexpected changes {changes:?}"),
                }
            }
        })
        .await
        .expect("filter did not receive the transaction");
        assert_eq!(hashes, [hash]);
    }

    #[tokio::test]
    async fn test_filter_poll_without_new_block_keeps_filter_alive() {
        let provider = MockEthProvider::default();
        provider.add_header(FixedBytes::random(), alloy_consensus::Header::default());
        let eth_filter = EthFilter::new(
            build_test_eth_api(provider),
            EthFilterConfig::default(),
            Runtime::test(),
        );
        let ttl = eth_filter.inner.stale_filter_ttl;
        let active_filters = eth_filter.active_filters();
        let ids = [
            eth_filter.new_block_filter().await.unwrap(),
            eth_filter.new_pending_transaction_filter(None).await.unwrap(),
        ];

        for id in &ids {
            // The first poll advances the filter past the head, so the second one sees no new
            // block.
            eth_filter.filter_changes(id.clone()).await.unwrap();
            // Unless the second poll refreshes it, the filter is stale at `now + ttl / 2`.
            active_filters.inner.lock().await.get_mut(id).unwrap().last_poll_timestamp -= ttl;
            eth_filter.filter_changes(id.clone()).await.unwrap();
        }

        eth_filter.clear_stale_filters(Instant::now() + ttl / 2).await;
        for id in &ids {
            assert!(active_filters.contains(id).await, "{id:?} was evicted");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn expired_filter_frees_its_slot_at_the_next_sweep() {
        let eth_api = build_test_eth_api(provider_with_head());
        let config = EthFilterConfig::default().max_active_filters(Some(1));
        let eth_filter = EthFilter::new(eth_api, config, Runtime::test());
        let ttl = eth_filter.inner.stale_filter_ttl;

        let id = eth_filter.new_block_filter().await.unwrap();
        eth_filter.active_filters().inner.lock().await.get_mut(&id).unwrap().last_poll_timestamp -=
            ttl;
        assert_eq!(eth_filter.new_block_filter().await.unwrap_err().code(), LIMIT_EXCEEDED_CODE);

        tokio::time::sleep(ttl / STALE_FILTER_SWEEPS_PER_TTL).await;
        tokio::task::yield_now().await;

        assert!(!eth_filter.active_filters().contains(&id).await);
        eth_filter.new_block_filter().await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn tiny_stale_filter_ttl_keeps_the_sweeper_running() {
        let config = EthFilterConfig::default().stale_filter_ttl(Duration::from_nanos(3));
        let eth_filter =
            EthFilter::new(build_test_eth_api(provider_with_head()), config, Runtime::test());

        // A zero sweep period would panic here instead of sweeping until the timeout.
        tokio::time::timeout(Duration::from_millis(10), eth_filter.watch_and_clear_stale_filters())
            .await
            .unwrap_err();
    }

    // Runs on one thread, so a poll that blocked the thread on the receiver would deadlock.
    #[tokio::test(flavor = "current_thread")]
    async fn concurrent_poll_of_a_pending_filter_yields() {
        let eth_filter = EthFilter::new(
            build_test_eth_api(provider_with_head()),
            EthFilterConfig::default(),
            Runtime::test(),
        );
        let id = eth_filter.new_pending_transaction_filter(None).await.unwrap();
        let PendingTransactionKind::Hashes(receiver) = pending_kind(&eth_filter, &id).await else {
            panic!("expected a hash filter")
        };

        let guard = receiver.txs_receiver.lock().await;
        let poll = tokio::spawn({
            let eth_filter = eth_filter.clone();
            async move { eth_filter.filter_changes(id).await }
        });
        tokio::task::yield_now().await;
        assert!(!poll.is_finished());

        drop(guard);
        assert_eq!(poll.await.unwrap().unwrap(), FilterChanges::Hashes(Vec::new()));
    }
}
