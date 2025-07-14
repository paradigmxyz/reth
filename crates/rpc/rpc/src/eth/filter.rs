//! `eth_` `Filter` RPC handler implementation

use alloy_consensus::BlockHeader;
use alloy_primitives::{Sealable, TxHash};
use alloy_rpc_types_eth::{
    BlockNumHash, Filter, FilterBlockOption, FilterChanges, FilterId, Log,
    PendingTransactionFilterKind,
};
use async_trait::async_trait;
use futures::future::TryFutureExt;
use jsonrpsee::{core::RpcResult, server::IdProvider};
use reth_errors::ProviderError;
use reth_primitives_traits::{NodePrimitives, SealedHeader};
use reth_rpc_eth_api::{
    EngineEthFilter, EthApiTypes, EthFilterApiServer, FullEthApiTypes, QueryLimits, RpcConvert,
    RpcNodeCore, RpcNodeCoreExt, RpcTransaction,
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
use reth_tasks::TaskSpawner;
use reth_transaction_pool::{NewSubpoolTransactionStream, PoolTransaction, TransactionPool};
use std::{
    collections::{HashMap, VecDeque},
    fmt,
    future::Future,
    iter::{Peekable, StepBy},
    ops::RangeInclusive,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    sync::{mpsc::Receiver, oneshot, Mutex},
    time::MissedTickBehavior,
};
use tracing::{debug, error, trace};

impl<Eth> EngineEthFilter for EthFilter<Eth>
where
    Eth: FullEthApiTypes + RpcNodeCoreExt<Provider: BlockIdReader> + 'static,
{
    /// Returns logs matching given filter object, no query limits
    fn logs(
        &self,
        filter: Filter,
        limits: QueryLimits,
    ) -> impl Future<Output = RpcResult<Vec<Log>>> + Send {
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

/// The maximum number of headers we read at once when handling a range filter.
const MAX_HEADERS_RANGE: u64 = 1_000; // with ~530bytes per header this is ~500kb

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
    /// use reth_tasks::TokioTaskExecutor;
    /// use reth_transaction_pool::noop::NoopTransactionPool;
    /// let eth_api = EthApi::builder(
    ///     NoopProvider::default(),
    ///     NoopTransactionPool::default(),
    ///     NoopNetwork::default(),
    ///     EthEvmConfig::mainnet(),
    /// )
    /// .build();
    /// let filter = EthFilter::new(eth_api, Default::default(), TokioTaskExecutor::default().boxed());
    /// ```
    pub fn new(eth_api: Eth, config: EthFilterConfig, task_spawner: Box<dyn TaskSpawner>) -> Self {
        let EthFilterConfig { max_blocks_per_filter, max_logs_per_response, stale_filter_ttl } =
            config;
        let inner = EthFilterInner {
            eth_api,
            active_filters: ActiveFilters::new(),
            id_provider: Arc::new(EthSubscriptionIdProvider::default()),
            max_headers_range: MAX_HEADERS_RANGE,
            task_spawner,
            stale_filter_ttl,
            query_limits: QueryLimits { max_blocks_per_filter, max_logs_per_response },
        };

        let eth_filter = Self { inner: Arc::new(inner) };

        let this = eth_filter.clone();
        eth_filter.inner.task_spawner.spawn_critical(
            "eth-filters_stale-filters-clean",
            Box::pin(async move {
                this.watch_and_clear_stale_filters().await;
            }),
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
        self.active_filters().inner.lock().await.retain(|id, filter| {
            let is_valid = (now - filter.last_poll_timestamp) < self.inner.stale_filter_ttl;

            if !is_valid {
                trace!(target: "rpc::eth", "evict filter with id: {:?}", id);
            }

            is_valid
        })
    }
}

impl<Eth> EthFilter<Eth>
where
    Eth: FullEthApiTypes<Provider: BlockReader + BlockIdReader> + RpcNodeCoreExt + 'static,
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
    ) -> Result<FilterChanges<RpcTransaction<Eth::NetworkTypes>>, EthFilterError> {
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
            FilterKind::PendingTransaction(filter) => Ok(filter.drain().await),
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
                        logs_utils::get_filter_block_range(from, to, start_block, info)
                    }
                    FilterBlockOption::AtBlockHash(_) => {
                        // blockHash is equivalent to fromBlock = toBlock = the block number with
                        // hash blockHash
                        // get_logs_in_block_range is inclusive
                        (start_block, best_number)
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
    pub async fn filter_logs(&self, id: FilterId) -> Result<Vec<Log>, EthFilterError> {
        let filter = {
            let filters = self.inner.active_filters.inner.lock().await;
            if let FilterKind::Log(ref filter) =
                filters.get(&id).ok_or_else(|| EthFilterError::FilterNotFound(id.clone()))?.kind
            {
                *filter.clone()
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
    ) -> Result<Vec<Log>, EthFilterError> {
        self.inner.clone().logs_for_filter(filter, limits).await
    }
}

#[async_trait]
impl<Eth> EthFilterApiServer<RpcTransaction<Eth::NetworkTypes>> for EthFilter<Eth>
where
    Eth: FullEthApiTypes
        + RpcNodeCoreExt<
            Provider: BlockIdReader,
            Primitives: NodePrimitives<
                SignedTx = <<Eth as RpcNodeCore>::Provider as TransactionsProvider>::Transaction,
            >,
        > + 'static,
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
                    self.inner.eth_api.tx_resp_builder().clone(),
                );
                FilterKind::PendingTransaction(PendingTransactionKind::FullTransaction(Arc::new(
                    full_txs_receiver,
                )))
            }
        };

        //let filter = FilterKind::PendingTransaction(transaction_kind);

        // Install the filter and propagate any errors
        self.inner.install_filter(transaction_kind).await
    }

    /// Handler for `eth_getFilterChanges`
    async fn filter_changes(
        &self,
        id: FilterId,
    ) -> RpcResult<FilterChanges<RpcTransaction<Eth::NetworkTypes>>> {
        trace!(target: "rpc::eth", "Serving eth_getFilterChanges");
        Ok(Self::filter_changes(self, id).await?)
    }

    /// Returns an array of all logs matching filter with given id.
    ///
    /// Returns an error if no matching log filter exists.
    ///
    /// Handler for `eth_getFilterLogs`
    async fn filter_logs(&self, id: FilterId) -> RpcResult<Vec<Log>> {
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
    async fn logs(&self, filter: Filter) -> RpcResult<Vec<Log>> {
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
    /// maximum number of headers to read at once for range filter
    max_headers_range: u64,
    /// The type that can spawn tasks.
    task_spawner: Box<dyn TaskSpawner>,
    /// Duration since the last filter poll, after which the filter is considered stale
    stale_filter_ttl: Duration,
}

impl<Eth> EthFilterInner<Eth>
where
    Eth: RpcNodeCoreExt<Provider: BlockIdReader, Pool: TransactionPool>
        + EthApiTypes<NetworkTypes: reth_rpc_eth_api::types::RpcTypes>
        + 'static,
{
    /// Access the underlying provider.
    fn provider(&self) -> &Eth::Provider {
        self.eth_api.provider()
    }

    /// Access the underlying [`EthStateCache`].
    fn eth_cache(
        &self,
    ) -> &EthStateCache<ProviderBlock<Eth::Provider>, ProviderReceipt<Eth::Provider>> {
        self.eth_api.cache()
    }

    /// Returns logs matching given filter object.
    async fn logs_for_filter(
        self: Arc<Self>,
        filter: Filter,
        limits: QueryLimits,
    ) -> Result<Vec<Log>, EthFilterError> {
        match filter.block_option {
            FilterBlockOption::AtBlockHash(block_hash) => {
                // for all matching logs in the block
                // get the block header with the hash
                let header = self
                    .provider()
                    .header_by_hash_or_number(block_hash.into())?
                    .ok_or_else(|| ProviderError::HeaderNotFound(block_hash.into()))?;

                let block_num_hash = BlockNumHash::new(header.number(), block_hash);

                // we also need to ensure that the receipts are available and return an error if
                // not, in case the block hash been reorged
                let (receipts, maybe_block) = self
                    .eth_cache()
                    .get_receipts_and_maybe_block(block_num_hash.hash)
                    .await?
                    .ok_or(EthApiError::HeaderNotFound(block_hash.into()))?;

                let mut all_logs = Vec::new();
                append_matching_block_logs(
                    &mut all_logs,
                    maybe_block
                        .map(ProviderOrBlock::Block)
                        .unwrap_or_else(|| ProviderOrBlock::Provider(self.provider())),
                    &filter,
                    block_num_hash,
                    &receipts,
                    false,
                    header.timestamp(),
                )?;

                Ok(all_logs)
            }
            FilterBlockOption::Range { from_block, to_block } => {
                // compute the range
                let info = self.provider().chain_info()?;

                // we start at the most recent block if unset in filter
                let start_block = info.best_number;
                let from = from_block
                    .map(|num| self.provider().convert_block_number(num))
                    .transpose()?
                    .flatten();
                let to = to_block
                    .map(|num| self.provider().convert_block_number(num))
                    .transpose()?
                    .flatten();
                let (from_block_number, to_block_number) =
                    logs_utils::get_filter_block_range(from, to, start_block, info);
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
    ) -> Result<Vec<Log>, EthFilterError> {
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

        let (tx, rx) = oneshot::channel();
        let this = self.clone();
        self.task_spawner.spawn_blocking(Box::pin(async move {
            let res =
                this.get_logs_in_block_range_inner(&filter, from_block, to_block, limits).await;
            let _ = tx.send(res);
        }));

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
    ) -> Result<Vec<Log>, EthFilterError> {
        let mut all_logs = Vec::new();
        let mut matching_headers = Vec::new();

        // get current chain tip to determine processing mode
        let chain_tip = self.provider().best_block_number()?;

        // first collect all headers that match the bloom filter for cached mode decision
        for (from, to) in
            BlockRangeInclusiveIter::new(from_block..=to_block, self.max_headers_range)
        {
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
            append_matching_block_logs(
                &mut all_logs,
                recovered_block
                    .map(ProviderOrBlock::Block)
                    .unwrap_or_else(|| ProviderOrBlock::Provider(self.provider())),
                filter,
                num_hash,
                &receipts,
                false,
                header.timestamp(),
            )?;

            // size check but only if range is multiple blocks, so we always return all
            // logs of a single block
            let is_multi_block_range = from_block != to_block;
            if let Some(max_logs_per_response) = limits.max_logs_per_response {
                if is_multi_block_range && all_logs.len() > max_logs_per_response {
                    debug!(
                        target: "rpc::eth::filter",
                        logs_found = all_logs.len(),
                        max_logs_per_response,
                        from_block,
                        to_block = num_hash.number.saturating_sub(1),
                        "Query exceeded max logs per response limit"
                    );
                    return Err(EthFilterError::QueryExceedsMaxResults {
                        max_logs: max_logs_per_response,
                        from_block,
                        to_block: num_hash.number.saturating_sub(1),
                    });
                }
            }
        }

        Ok(all_logs)
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
    tx_resp_builder: TxCompat,
}

impl<T, TxCompat> FullTransactionsReceiver<T, TxCompat>
where
    T: PoolTransaction + 'static,
    TxCompat: RpcConvert<Primitives: NodePrimitives<SignedTx = T::Consensus>>,
{
    /// Creates a new `FullTransactionsReceiver` encapsulating the provided transaction stream.
    fn new(stream: NewSubpoolTransactionStream<T>, tx_resp_builder: TxCompat) -> Self {
        Self { txs_stream: Arc::new(Mutex::new(stream)), tx_resp_builder }
    }

    /// Returns all new pending transactions received since the last poll.
    async fn drain(&self) -> FilterChanges<RpcTransaction<TxCompat::Network>> {
        let mut pending_txs = Vec::new();
        let mut prepared_stream = self.txs_stream.lock().await;

        while let Ok(tx) = prepared_stream.try_recv() {
            match self.tx_resp_builder.fill_pending(tx.transaction.to_consensus()) {
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
    TxCompat: RpcConvert<Primitives: NodePrimitives<SignedTx = T::Consensus>> + 'static,
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
    /// Query scope is too broad.
    #[error("query exceeds max block range {0}")]
    QueryExceedsMaxBlocks(u64),
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
            EthFilterError::FilterNotFound(_) => rpc_error_with_code(
                jsonrpsee::types::error::INVALID_PARAMS_CODE,
                "filter not found",
            ),
            err @ EthFilterError::InternalError => {
                rpc_error_with_code(jsonrpsee::types::error::INTERNAL_ERROR_CODE, err.to_string())
            }
            EthFilterError::EthAPIError(err) => err.into(),
            err @ (EthFilterError::InvalidBlockRangeParams |
            EthFilterError::QueryExceedsMaxBlocks(_) |
            EthFilterError::QueryExceedsMaxResults { .. }) => {
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

/// Helper type for the common pattern of returning receipts, block and the original header that is
/// a match for the filter.
struct ReceiptBlockResult<P>
where
    P: ReceiptProvider + BlockReader,
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
    Eth: RpcNodeCoreExt<Provider: BlockIdReader, Pool: TransactionPool> + EthApiTypes + 'static,
> {
    /// Use cache-based processing for recent blocks
    Cached(CachedMode<Eth>),
    /// Use range-based processing for older blocks
    Range(RangeBlockMode<Eth>),
}

impl<
        Eth: RpcNodeCoreExt<Provider: BlockIdReader, Pool: TransactionPool> + EthApiTypes + 'static,
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

        if use_cached_mode && !sealed_headers.is_empty() {
            Self::Cached(CachedMode { filter_inner, headers_iter: sealed_headers.into_iter() })
        } else {
            Self::Range(RangeBlockMode {
                filter_inner,
                iter: sealed_headers.into_iter().peekable(),
                next: VecDeque::new(),
                max_range: max_headers_range as usize,
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
    Eth: RpcNodeCoreExt<Provider: BlockIdReader, Pool: TransactionPool> + EthApiTypes + 'static,
> {
    filter_inner: Arc<EthFilterInner<Eth>>,
    headers_iter: std::vec::IntoIter<SealedHeader<<Eth::Provider as HeaderProvider>::Header>>,
}

impl<
        Eth: RpcNodeCoreExt<Provider: BlockIdReader, Pool: TransactionPool> + EthApiTypes + 'static,
    > CachedMode<Eth>
{
    async fn next(&mut self) -> Result<Option<ReceiptBlockResult<Eth::Provider>>, EthFilterError> {
        for header in self.headers_iter.by_ref() {
            // Use get_receipts_and_maybe_block which has automatic fallback to provider
            if let Some((receipts, maybe_block)) =
                self.filter_inner.eth_cache().get_receipts_and_maybe_block(header.hash()).await?
            {
                return Ok(Some(ReceiptBlockResult {
                    receipts,
                    recovered_block: maybe_block,
                    header,
                }));
            }
        }

        Ok(None) // No more headers
    }
}

/// Mode for processing blocks using range queries for older blocks
struct RangeBlockMode<
    Eth: RpcNodeCoreExt<Provider: BlockIdReader, Pool: TransactionPool> + EthApiTypes + 'static,
> {
    filter_inner: Arc<EthFilterInner<Eth>>,
    iter: Peekable<std::vec::IntoIter<SealedHeader<<Eth::Provider as HeaderProvider>::Header>>>,
    next: VecDeque<ReceiptBlockResult<Eth::Provider>>,
    max_range: usize,
}

impl<
        Eth: RpcNodeCoreExt<Provider: BlockIdReader, Pool: TransactionPool> + EthApiTypes + 'static,
    > RangeBlockMode<Eth>
{
    async fn next(&mut self) -> Result<Option<ReceiptBlockResult<Eth::Provider>>, EthFilterError> {
        if let Some(result) = self.next.pop_front() {
            return Ok(Some(result));
        }

        let Some(next_header) = self.iter.next() else {
            return Ok(None);
        };

        let mut range_headers = Vec::with_capacity(self.max_range);
        range_headers.push(next_header);

        // Collect consecutive blocks up to max_range size
        while range_headers.len() < self.max_range {
            let Some(peeked) = self.iter.peek() else { break };
            let Some(last_header) = range_headers.last() else { break };

            let expected_next = last_header.header().number() + 1;
            if peeked.header().number() != expected_next {
                break; // Non-consecutive block, stop here
            }

            let Some(next_header) = self.iter.next() else { break };
            range_headers.push(next_header);
        }

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
                    // Not cached - fetch directly from provider without queuing
                    match self.filter_inner.provider().receipts_by_block(header.hash().into())? {
                        Some(receipts) => Arc::new(receipts),
                        None => continue, // No receipts found
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{eth::EthApi, EthApiBuilder};
    use alloy_primitives::FixedBytes;
    use rand::Rng;
    use reth_chainspec::ChainSpecProvider;
    use reth_ethereum_primitives::TxType;
    use reth_evm_ethereum::EthEvmConfig;
    use reth_network_api::noop::NoopNetwork;
    use reth_provider::test_utils::MockEthProvider;
    use reth_tasks::TokioTaskExecutor;
    use reth_testing_utils::generators;
    use reth_transaction_pool::test_utils::{testing_pool, TestPool};
    use std::{collections::VecDeque, sync::Arc};

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
    fn build_test_eth_api(
        provider: MockEthProvider,
    ) -> EthApi<MockEthProvider, TestPool, NoopNetwork, EthEvmConfig> {
        EthApiBuilder::new(
            provider.clone(),
            testing_pool(),
            NoopNetwork::default(),
            EthEvmConfig::new(provider.chain_spec()),
        )
        .build()
    }

    #[tokio::test]
    async fn test_range_block_mode_empty_range() {
        let provider = MockEthProvider::default();
        let eth_api = build_test_eth_api(provider);

        let eth_filter = super::EthFilter::new(
            eth_api,
            EthFilterConfig::default(),
            Box::new(TokioTaskExecutor::default()),
        );
        let filter_inner = eth_filter.inner;

        let headers = vec![];
        let max_range = 100;

        let mut range_mode = RangeBlockMode {
            filter_inner,
            iter: headers.into_iter().peekable(),
            next: VecDeque::new(),
            max_range,
        };

        let result = range_mode.next().await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_range_block_mode_queued_results_priority() {
        let provider = MockEthProvider::default();
        let eth_api = build_test_eth_api(provider);

        let eth_filter = super::EthFilter::new(
            eth_api,
            EthFilterConfig::default(),
            Box::new(TokioTaskExecutor::default()),
        );
        let filter_inner = eth_filter.inner;

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

        // create specific mock results to test ordering
        let expected_block_hash_1 = FixedBytes::from([1u8; 32]);
        let expected_block_hash_2 = FixedBytes::from([2u8; 32]);

        // create mock receipts to test receipt handling
        let mock_receipt_1 = reth_ethereum_primitives::Receipt {
            tx_type: TxType::Legacy,
            cumulative_gas_used: 100_000,
            logs: vec![],
            success: true,
        };
        let mock_receipt_2 = reth_ethereum_primitives::Receipt {
            tx_type: TxType::Eip1559,
            cumulative_gas_used: 200_000,
            logs: vec![],
            success: true,
        };
        let mock_receipt_3 = reth_ethereum_primitives::Receipt {
            tx_type: TxType::Eip2930,
            cumulative_gas_used: 150_000,
            logs: vec![],
            success: false, // Different success status
        };

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
        };

        // first call should return the first queued result (FIFO order)
        let result1 = range_mode.next().await;
        assert!(result1.is_ok());
        let receipt_result1 = result1.unwrap().unwrap();
        assert_eq!(receipt_result1.header.hash(), expected_block_hash_1);
        assert_eq!(receipt_result1.header.number, 42);

        // verify receipts
        assert_eq!(receipt_result1.receipts.len(), 2);
        assert_eq!(receipt_result1.receipts[0].tx_type, mock_receipt_1.tx_type);
        assert_eq!(
            receipt_result1.receipts[0].cumulative_gas_used,
            mock_receipt_1.cumulative_gas_used
        );
        assert_eq!(receipt_result1.receipts[0].success, mock_receipt_1.success);
        assert_eq!(receipt_result1.receipts[1].tx_type, mock_receipt_2.tx_type);
        assert_eq!(
            receipt_result1.receipts[1].cumulative_gas_used,
            mock_receipt_2.cumulative_gas_used
        );
        assert_eq!(receipt_result1.receipts[1].success, mock_receipt_2.success);

        // second call should return the second queued result
        let result2 = range_mode.next().await;
        assert!(result2.is_ok());
        let receipt_result2 = result2.unwrap().unwrap();
        assert_eq!(receipt_result2.header.hash(), expected_block_hash_2);
        assert_eq!(receipt_result2.header.number, 43);

        // verify receipts
        assert_eq!(receipt_result2.receipts.len(), 1);
        assert_eq!(receipt_result2.receipts[0].tx_type, mock_receipt_3.tx_type);
        assert_eq!(
            receipt_result2.receipts[0].cumulative_gas_used,
            mock_receipt_3.cumulative_gas_used
        );
        assert_eq!(receipt_result2.receipts[0].success, mock_receipt_3.success);

        // queue should now be empty
        assert!(range_mode.next.is_empty());

        let result3 = range_mode.next().await;
        assert!(result3.is_ok());
    }

    #[tokio::test]
    async fn test_range_block_mode_single_block_no_receipts() {
        let provider = MockEthProvider::default();
        let eth_api = build_test_eth_api(provider);

        let eth_filter = super::EthFilter::new(
            eth_api,
            EthFilterConfig::default(),
            Box::new(TokioTaskExecutor::default()),
        );
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
        };

        let result = range_mode.next().await;
        assert!(result.is_ok());
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

        let receipt_100_1 = reth_ethereum_primitives::Receipt {
            tx_type: TxType::Legacy,
            cumulative_gas_used: 21_000,
            logs: vec![mock_log.clone()],
            success: true,
        };
        let receipt_100_2 = reth_ethereum_primitives::Receipt {
            tx_type: TxType::Eip1559,
            cumulative_gas_used: 42_000,
            logs: vec![mock_log.clone()],
            success: true,
        };
        let receipt_101_1 = reth_ethereum_primitives::Receipt {
            tx_type: TxType::Eip2930,
            cumulative_gas_used: 30_000,
            logs: vec![mock_log.clone()],
            success: false,
        };

        provider.add_receipts(100, vec![receipt_100_1.clone(), receipt_100_2.clone()]);
        provider.add_receipts(101, vec![receipt_101_1.clone()]);

        let eth_api = build_test_eth_api(provider);

        let eth_filter = super::EthFilter::new(
            eth_api,
            EthFilterConfig::default(),
            Box::new(TokioTaskExecutor::default()),
        );
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
        };

        // first call should fetch receipts from provider and return first block with receipts
        let result = range_mode.next().await;
        assert!(result.is_ok());
        let receipt_result = result.unwrap().unwrap();

        assert_eq!(receipt_result.header.hash(), block_hash_1);
        assert_eq!(receipt_result.header.number, 100);
        assert_eq!(receipt_result.receipts.len(), 2);

        // verify receipts
        assert_eq!(receipt_result.receipts[0].tx_type, receipt_100_1.tx_type);
        assert_eq!(
            receipt_result.receipts[0].cumulative_gas_used,
            receipt_100_1.cumulative_gas_used
        );
        assert_eq!(receipt_result.receipts[0].success, receipt_100_1.success);

        assert_eq!(receipt_result.receipts[1].tx_type, receipt_100_2.tx_type);
        assert_eq!(
            receipt_result.receipts[1].cumulative_gas_used,
            receipt_100_2.cumulative_gas_used
        );
        assert_eq!(receipt_result.receipts[1].success, receipt_100_2.success);

        // second call should return the second block with receipts
        let result2 = range_mode.next().await;
        assert!(result2.is_ok());
        let receipt_result2 = result2.unwrap().unwrap();

        assert_eq!(receipt_result2.header.hash(), block_hash_2);
        assert_eq!(receipt_result2.header.number, 101);
        assert_eq!(receipt_result2.receipts.len(), 1);

        // verify receipts
        assert_eq!(receipt_result2.receipts[0].tx_type, receipt_101_1.tx_type);
        assert_eq!(
            receipt_result2.receipts[0].cumulative_gas_used,
            receipt_101_1.cumulative_gas_used
        );
        assert_eq!(receipt_result2.receipts[0].success, receipt_101_1.success);

        // third call should return None since no more blocks with receipts
        let result3 = range_mode.next().await;
        assert!(result3.is_ok());
        assert!(result3.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_range_block_mode_iterator_exhaustion() {
        let provider = MockEthProvider::default();
        let eth_api = build_test_eth_api(provider);

        let eth_filter = super::EthFilter::new(
            eth_api,
            EthFilterConfig::default(),
            Box::new(TokioTaskExecutor::default()),
        );
        let filter_inner = eth_filter.inner;

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

        let mut range_mode = RangeBlockMode {
            filter_inner,
            iter: headers.into_iter().peekable(),
            next: VecDeque::new(),
            max_range: 1,
        };

        let result1 = range_mode.next().await;
        assert!(result1.is_ok());

        assert!(range_mode.iter.peek().is_some());

        let result2 = range_mode.next().await;
        assert!(result2.is_ok());

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
        let eth_filter = super::EthFilter::new(
            eth_api,
            EthFilterConfig::default(),
            Box::new(TokioTaskExecutor::default()),
        );
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

        let eth_filter = super::EthFilter::new(
            eth_api,
            EthFilterConfig::default(),
            Box::new(TokioTaskExecutor::default()),
        );
        let filter_inner = eth_filter.inner;

        let headers: Vec<SealedHeader<alloy_consensus::Header>> = vec![];

        let mut cached_mode = CachedMode { filter_inner, headers_iter: headers.into_iter() };

        // should immediately return None for empty headers
        let result = cached_mode.next().await.expect("next should succeed");
        assert!(result.is_none());
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
                    alloy_primitives::Bloom::from([1u8; 256])
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
        let eth_filter = EthFilter::new(
            eth_api,
            EthFilterConfig::default(),
            Box::new(TokioTaskExecutor::default()),
        );

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
}
