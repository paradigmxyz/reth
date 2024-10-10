//! `eth_` `Filter` RPC handler implementation

use std::{
    collections::HashMap,
    fmt,
    iter::StepBy,
    marker::PhantomData,
    ops::RangeInclusive,
    sync::Arc,
    time::{Duration, Instant},
};

use alloy_primitives::TxHash;
use alloy_rpc_types::{
    BlockNumHash, Filter, FilterBlockOption, FilterChanges, FilterId, FilteredParams, Log,
    PendingTransactionFilterKind,
};
use async_trait::async_trait;
use jsonrpsee::{core::RpcResult, server::IdProvider};
use reth_chainspec::ChainInfo;
use reth_node_api::EthApiTypes;
use reth_primitives::{Receipt, SealedBlock, TransactionSignedEcRecovered};
use reth_provider::{BlockIdReader, BlockReader, EvmEnvProvider, ProviderError};
use reth_rpc_eth_api::{EthFilterApiServer, FullEthApiTypes, RpcTransaction, TransactionCompat};
use reth_rpc_eth_types::{
    logs_utils::{self, append_matching_block_logs, ProviderOrBlock},
    EthApiError, EthFilterConfig, EthStateCache, EthSubscriptionIdProvider,
};
use reth_rpc_server_types::{result::rpc_error_with_code, ToRpcResult};
use reth_rpc_types_compat::transaction::from_recovered;
use reth_tasks::TaskSpawner;
use reth_transaction_pool::{NewSubpoolTransactionStream, PoolTransaction, TransactionPool};
use tokio::{
    sync::{mpsc::Receiver, Mutex},
    time::MissedTickBehavior,
};
use tracing::trace;

/// The maximum number of headers we read at once when handling a range filter.
const MAX_HEADERS_RANGE: u64 = 1_000; // with ~530bytes per header this is ~500kb

/// `Eth` filter RPC implementation.
pub struct EthFilter<Provider, Pool, Eth: EthApiTypes> {
    /// All nested fields bundled together
    inner: Arc<EthFilterInner<Provider, Pool, RpcTransaction<Eth::NetworkTypes>>>,
    /// Assembles response data w.r.t. network.
    _tx_resp_builder: PhantomData<Eth>,
}

impl<Provider, Pool, Eth> Clone for EthFilter<Provider, Pool, Eth>
where
    Eth: EthApiTypes,
{
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone(), _tx_resp_builder: PhantomData }
    }
}

impl<Provider, Pool, Eth> EthFilter<Provider, Pool, Eth>
where
    Provider: Send + Sync + 'static,
    Pool: Send + Sync + 'static,
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
    pub fn new(
        provider: Provider,
        pool: Pool,
        eth_cache: EthStateCache,
        config: EthFilterConfig,
        task_spawner: Box<dyn TaskSpawner>,
    ) -> Self {
        let EthFilterConfig { max_blocks_per_filter, max_logs_per_response, stale_filter_ttl } =
            config;
        let inner = EthFilterInner {
            provider,
            active_filters: ActiveFilters::new(),
            pool,
            id_provider: Arc::new(EthSubscriptionIdProvider::default()),
            eth_cache,
            max_headers_range: MAX_HEADERS_RANGE,
            task_spawner,
            stale_filter_ttl,
            // if not set, use the max value, which is effectively no limit
            max_blocks_per_filter: max_blocks_per_filter.unwrap_or(u64::MAX),
            max_logs_per_response: max_logs_per_response.unwrap_or(usize::MAX),
        };

        let eth_filter = Self { inner: Arc::new(inner), _tx_resp_builder: PhantomData };

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

impl<Provider, Pool, Eth> EthFilter<Provider, Pool, Eth>
where
    Provider: BlockReader + BlockIdReader + EvmEnvProvider + 'static,
    Pool: TransactionPool + 'static,
    <Pool as TransactionPool>::Transaction: 'static,
    Eth: FullEthApiTypes,
{
    /// Returns all the filter changes for the given id, if any
    pub async fn filter_changes(
        &self,
        id: FilterId,
    ) -> Result<FilterChanges<RpcTransaction<Eth::NetworkTypes>>, EthFilterError> {
        let info = self.inner.provider.chain_info()?;
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
                    self.inner.provider.canonical_hashes_range(start_block, end_block).map_err(
                        |_| EthApiError::HeaderRangeNotFound(start_block.into(), end_block.into()),
                    )?;
                Ok(FilterChanges::Hashes(block_hashes))
            }
            FilterKind::Log(filter) => {
                let (from_block_number, to_block_number) = match filter.block_option {
                    FilterBlockOption::Range { from_block, to_block } => {
                        let from = from_block
                            .map(|num| self.inner.provider.convert_block_number(num))
                            .transpose()?
                            .flatten();
                        let to = to_block
                            .map(|num| self.inner.provider.convert_block_number(num))
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
                    .get_logs_in_block_range(&filter, from_block_number, to_block_number, info)
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

        self.inner.logs_for_filter(filter).await
    }
}

#[async_trait]
impl<Provider, Pool, Eth> EthFilterApiServer<RpcTransaction<Eth::NetworkTypes>>
    for EthFilter<Provider, Pool, Eth>
where
    Provider: BlockReader + BlockIdReader + EvmEnvProvider + 'static,
    Pool: TransactionPool + 'static,
    Eth: FullEthApiTypes + 'static,
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
                let receiver = self.inner.pool.pending_transactions_listener();
                let pending_txs_receiver = PendingTransactionsReceiver::new(receiver);
                FilterKind::PendingTransaction(PendingTransactionKind::Hashes(pending_txs_receiver))
            }
            PendingTransactionFilterKind::Full => {
                let stream = self.inner.pool.new_pending_pool_transactions_listener();
                let full_txs_receiver =
                    FullTransactionsReceiver::<_, Eth::TransactionCompat>::new(stream);
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
        Ok(self.inner.logs_for_filter(filter).await?)
    }
}

impl<Provider, Pool, Eth> std::fmt::Debug for EthFilter<Provider, Pool, Eth>
where
    Eth: EthApiTypes,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EthFilter").finish_non_exhaustive()
    }
}

/// Container type `EthFilter`
#[derive(Debug)]
struct EthFilterInner<Provider, Pool, Tx> {
    /// The transaction pool.
    pool: Pool,
    /// The provider that can interact with the chain.
    provider: Provider,
    /// All currently installed filters.
    active_filters: ActiveFilters<Tx>,
    /// Provides ids to identify filters
    id_provider: Arc<dyn IdProvider>,
    /// Maximum number of blocks that could be scanned per filter
    max_blocks_per_filter: u64,
    /// Maximum number of logs that can be returned in a response
    max_logs_per_response: usize,
    /// The async cache frontend for eth related data
    eth_cache: EthStateCache,
    /// maximum number of headers to read at once for range filter
    max_headers_range: u64,
    /// The type that can spawn tasks.
    task_spawner: Box<dyn TaskSpawner>,
    /// Duration since the last filter poll, after which the filter is considered stale
    stale_filter_ttl: Duration,
}

impl<Provider, Pool, Tx> EthFilterInner<Provider, Pool, Tx>
where
    Provider: BlockReader + BlockIdReader + EvmEnvProvider + 'static,
    Pool: TransactionPool + 'static,
{
    /// Returns logs matching given filter object.
    async fn logs_for_filter(&self, filter: Filter) -> Result<Vec<Log>, EthFilterError> {
        match filter.block_option {
            FilterBlockOption::AtBlockHash(block_hash) => {
                // for all matching logs in the block
                // get the block header with the hash
                let header = self
                    .provider
                    .header_by_hash_or_number(block_hash.into())?
                    .ok_or_else(|| ProviderError::HeaderNotFound(block_hash.into()))?;

                let block_num_hash = BlockNumHash::new(header.number, block_hash);

                // we also need to ensure that the receipts are available and return an error if
                // not, in case the block hash been reorged
                let (receipts, maybe_block) = self
                    .receipts_and_maybe_block(
                        &block_num_hash,
                        self.provider.chain_info()?.best_number,
                    )
                    .await?
                    .ok_or(EthApiError::HeaderNotFound(block_hash.into()))?;

                let mut all_logs = Vec::new();
                append_matching_block_logs(
                    &mut all_logs,
                    maybe_block
                        .map(|b| ProviderOrBlock::Block(b))
                        .unwrap_or_else(|| ProviderOrBlock::Provider(&self.provider)),
                    &FilteredParams::new(Some(filter)),
                    block_num_hash,
                    &receipts,
                    false,
                    header.timestamp,
                )?;

                Ok(all_logs)
            }
            FilterBlockOption::Range { from_block, to_block } => {
                // compute the range
                let info = self.provider.chain_info()?;

                // we start at the most recent block if unset in filter
                let start_block = info.best_number;
                let from = from_block
                    .map(|num| self.provider.convert_block_number(num))
                    .transpose()?
                    .flatten();
                let to = to_block
                    .map(|num| self.provider.convert_block_number(num))
                    .transpose()?
                    .flatten();
                let (from_block_number, to_block_number) =
                    logs_utils::get_filter_block_range(from, to, start_block, info);
                self.get_logs_in_block_range(&filter, from_block_number, to_block_number, info)
                    .await
            }
        }
    }

    /// Installs a new filter and returns the new identifier.
    async fn install_filter(&self, kind: FilterKind<Tx>) -> RpcResult<FilterId> {
        let last_poll_block_number = self.provider.best_block_number().to_rpc_result()?;
        let id = FilterId::from(self.id_provider.next_id());
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
        &self,
        filter: &Filter,
        from_block: u64,
        to_block: u64,
        chain_info: ChainInfo,
    ) -> Result<Vec<Log>, EthFilterError> {
        trace!(target: "rpc::eth::filter", from=from_block, to=to_block, ?filter, "finding logs in range");

        if to_block < from_block {
            return Err(EthFilterError::InvalidBlockRangeParams)
        }

        if to_block - from_block > self.max_blocks_per_filter {
            return Err(EthFilterError::QueryExceedsMaxBlocks(self.max_blocks_per_filter))
        }

        let mut all_logs = Vec::new();
        let filter_params = FilteredParams::new(Some(filter.clone()));

        // derive bloom filters from filter input, so we can check headers for matching logs
        let address_filter = FilteredParams::address_filter(&filter.address);
        let topics_filter = FilteredParams::topics_filter(&filter.topics);

        // loop over the range of new blocks and check logs if the filter matches the log's bloom
        // filter
        for (from, to) in
            BlockRangeInclusiveIter::new(from_block..=to_block, self.max_headers_range)
        {
            let headers = self.provider.headers_range(from..=to)?;

            for (idx, header) in headers.iter().enumerate() {
                // only if filter matches
                if FilteredParams::matches_address(header.logs_bloom, &address_filter) &&
                    FilteredParams::matches_topics(header.logs_bloom, &topics_filter)
                {
                    // these are consecutive headers, so we can use the parent hash of the next
                    // block to get the current header's hash
                    let block_hash = match headers.get(idx + 1) {
                        Some(parent) => parent.parent_hash,
                        None => self
                            .provider
                            .block_hash(header.number)?
                            .ok_or_else(|| ProviderError::HeaderNotFound(header.number.into()))?,
                    };

                    let num_hash = BlockNumHash::new(header.number, block_hash);
                    if let Some((receipts, maybe_block)) =
                        self.receipts_and_maybe_block(&num_hash, chain_info.best_number).await?
                    {
                        append_matching_block_logs(
                            &mut all_logs,
                            maybe_block
                                .map(|block| ProviderOrBlock::Block(block))
                                .unwrap_or_else(|| ProviderOrBlock::Provider(&self.provider)),
                            &filter_params,
                            num_hash,
                            &receipts,
                            false,
                            header.timestamp,
                        )?;

                        // size check but only if range is multiple blocks, so we always return all
                        // logs of a single block
                        let is_multi_block_range = from_block != to_block;
                        if is_multi_block_range && all_logs.len() > self.max_logs_per_response {
                            return Err(EthFilterError::QueryExceedsMaxResults(
                                self.max_logs_per_response,
                            ))
                        }
                    }
                }
            }
        }

        Ok(all_logs)
    }

    /// Retrieves receipts and block from cache if near the tip (4 blocks), otherwise only receipts.
    async fn receipts_and_maybe_block(
        &self,
        block_num_hash: &BlockNumHash,
        best_number: u64,
    ) -> Result<Option<(Arc<Vec<Receipt>>, Option<SealedBlock>)>, EthFilterError> {
        // The last 4 blocks are most likely cached, so we can just fetch them
        let cached_range = best_number.saturating_sub(4)..=best_number;
        let receipts_block = if cached_range.contains(&block_num_hash.number) {
            self.eth_cache
                .get_block_and_receipts(block_num_hash.hash)
                .await?
                .map(|(b, r)| (r, Some(b)))
        } else {
            self.eth_cache.get_receipts(block_num_hash.hash).await?.map(|r| (r, None))
        };
        Ok(receipts_block)
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
    _tx_resp_builder: PhantomData<TxCompat>,
}

impl<T, TxCompat> FullTransactionsReceiver<T, TxCompat>
where
    T: PoolTransaction + 'static,
    TxCompat: TransactionCompat,
{
    /// Creates a new `FullTransactionsReceiver` encapsulating the provided transaction stream.
    fn new(stream: NewSubpoolTransactionStream<T>) -> Self {
        Self { txs_stream: Arc::new(Mutex::new(stream)), _tx_resp_builder: PhantomData }
    }

    /// Returns all new pending transactions received since the last poll.
    async fn drain(&self) -> FilterChanges<TxCompat::Transaction>
    where
        T: PoolTransaction<Consensus: Into<TransactionSignedEcRecovered>>,
    {
        let mut pending_txs = Vec::new();
        let mut prepared_stream = self.txs_stream.lock().await;

        while let Ok(tx) = prepared_stream.try_recv() {
            pending_txs.push(from_recovered::<TxCompat>(tx.transaction.to_recovered_transaction()))
        }
        FilterChanges::Transactions(pending_txs)
    }
}

/// Helper trait for [FullTransactionsReceiver] to erase the `Transaction` type.
#[async_trait]
trait FullTransactionsFilter<T>: fmt::Debug + Send + Sync + Unpin + 'static {
    async fn drain(&self) -> FilterChanges<T>;
}

#[async_trait]
impl<T, TxCompat> FullTransactionsFilter<TxCompat::Transaction>
    for FullTransactionsReceiver<T, TxCompat>
where
    T: PoolTransaction<Consensus: Into<TransactionSignedEcRecovered>> + 'static,
    TxCompat: TransactionCompat + 'static,
{
    async fn drain(&self) -> FilterChanges<TxCompat::Transaction> {
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
    #[error("query exceeds max results {0}")]
    QueryExceedsMaxResults(usize),
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
            EthFilterError::QueryExceedsMaxResults(_)) => {
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

#[cfg(test)]
mod tests {
    use super::*;
    use rand::Rng;
    use reth_testing_utils::generators;

    #[test]
    fn test_block_range_iter() {
        let mut rng = generators::rng();

        let start = rng.gen::<u32>() as u64;
        let end = start.saturating_add(rng.gen::<u32>() as u64);
        let step = rng.gen::<u16>() as u64;
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
}
