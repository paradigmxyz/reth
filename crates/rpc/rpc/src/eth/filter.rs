//! Implementation of `eth` filter RPC.

use super::cache::EthStateCache;
use crate::{
    eth::{
        error::{EthApiError, EthResult},
        logs_utils,
    },
    result::{rpc_error_with_code, ToRpcResult},
    EthSubscriptionIdProvider,
};
use alloy_primitives::{Address, BlockNumber, B256};
use async_trait::async_trait;
use core::fmt;
use jsonrpsee::{core::RpcResult, server::IdProvider};
use reth_primitives::{
    BlockHashOrNumber, IntegerList, IntoRecoveredTransaction, Receipt, SealedBlock, TxHash,
};
use reth_provider::{BlockIdReader, BlockReader, EvmEnvProvider, LogHistoryReader, ProviderError};
use reth_rpc_api::EthFilterApiServer;
use reth_rpc_types::{
    Filter, FilterBlockOption, FilterChanges, FilterId, FilteredParams, Log,
    PendingTransactionFilterKind, ValueOrArray,
};
use reth_tasks::TaskSpawner;
use reth_transaction_pool::{NewSubpoolTransactionStream, PoolTransaction, TransactionPool};
use std::{
    collections::HashMap,
    ops::RangeInclusive,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    sync::{mpsc::Receiver, Mutex},
    time::MissedTickBehavior,
};
use tracing::trace;

/// `Eth` filter RPC implementation.
pub struct EthFilter<Provider, Pool> {
    /// All nested fields bundled together
    inner: Arc<EthFilterInner<Provider, Pool>>,
}

impl<Provider, Pool> EthFilter<Provider, Pool>
where
    Provider: Send + Sync + 'static,
    Pool: Send + Sync + 'static,
{
    /// Creates a new, shareable instance.
    ///
    /// This uses the given pool to get notified about new transactions, the provider to interact
    /// with the blockchain, the cache to fetch cacheable data, like the logs.
    ///
    /// See also [EthFilterConfig].
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
            active_filters: Default::default(),
            pool,
            id_provider: Arc::new(EthSubscriptionIdProvider::default()),
            eth_cache,
            task_spawner,
            stale_filter_ttl,
            // if not set, use the max value, which is effectively no limit
            max_blocks_per_filter: max_blocks_per_filter.unwrap_or(u64::MAX),
            max_logs_per_response: max_logs_per_response.unwrap_or(usize::MAX),
        };

        let eth_filter = Self { inner: Arc::new(inner) };

        let this = eth_filter.clone();
        eth_filter.inner.task_spawner.clone().spawn_critical(
            "eth-filters_stale-filters-clean",
            Box::pin(async move {
                this.watch_and_clear_stale_filters().await;
            }),
        );

        eth_filter
    }

    /// Returns all currently active filters
    pub fn active_filters(&self) -> &ActiveFilters {
        &self.inner.active_filters
    }

    /// Endless future that [Self::clear_stale_filters] every `stale_filter_ttl` interval.
    async fn watch_and_clear_stale_filters(&self) {
        let mut interval = tokio::time::interval(self.inner.stale_filter_ttl);
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

impl<Provider, Pool> EthFilter<Provider, Pool>
where
    Provider: BlockReader + BlockIdReader + EvmEnvProvider + LogHistoryReader + 'static,
    Pool: TransactionPool + 'static,
    <Pool as TransactionPool>::Transaction: 'static,
{
    /// Returns all the filter changes for the given id, if any
    pub async fn filter_changes(&self, id: FilterId) -> Result<FilterChanges, FilterError> {
        let info = self.inner.provider.chain_info()?;
        let best_number = info.best_number;

        // start_block is the block from which we should start fetching changes, the next block from
        // the last time changes were polled, in other words the best block at last poll + 1
        let (start_block, kind) = {
            let mut filters = self.inner.active_filters.inner.lock().await;
            let filter = filters.get_mut(&id).ok_or(FilterError::FilterNotFound(id))?;

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
                let block_hashes = self
                    .inner
                    .provider
                    .canonical_hashes_range(start_block, end_block)
                    .map_err(|_| EthApiError::UnknownBlockNumber)?;
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
                    .get_logs_in_block_range(&filter, from_block_number, to_block_number)
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
    pub async fn filter_logs(&self, id: FilterId) -> Result<FilterChanges, FilterError> {
        let filter = {
            let filters = self.inner.active_filters.inner.lock().await;
            if let FilterKind::Log(ref filter) =
                filters.get(&id).ok_or_else(|| FilterError::FilterNotFound(id.clone()))?.kind
            {
                *filter.clone()
            } else {
                // Not a log filter
                return Err(FilterError::FilterNotFound(id))
            }
        };

        let logs = self.inner.logs_for_filter(filter).await?;
        Ok(FilterChanges::Logs(logs))
    }
}

#[async_trait]
impl<Provider, Pool> EthFilterApiServer for EthFilter<Provider, Pool>
where
    Provider: BlockReader + BlockIdReader + EvmEnvProvider + LogHistoryReader + 'static,
    Pool: TransactionPool + 'static,
{
    /// Handler for `eth_newFilter`
    async fn new_filter(&self, filter: Filter) -> RpcResult<FilterId> {
        trace!(target: "rpc::eth", "Serving eth_newFilter");
        self.inner.install_filter(FilterKind::Log(Box::new(filter))).await
    }

    /// Handler for `eth_newBlockFilter`
    async fn new_block_filter(&self) -> RpcResult<FilterId> {
        trace!(target: "rpc::eth", "Serving eth_newBlockFilter");
        self.inner.install_filter(FilterKind::Block).await
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
                let full_txs_receiver = FullTransactionsReceiver::new(stream);
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
    async fn filter_changes(&self, id: FilterId) -> RpcResult<FilterChanges> {
        trace!(target: "rpc::eth", "Serving eth_getFilterChanges");
        Ok(EthFilter::filter_changes(self, id).await?)
    }

    /// Returns an array of all logs matching filter with given id.
    ///
    /// Returns an error if no matching log filter exists.
    ///
    /// Handler for `eth_getFilterLogs`
    async fn filter_logs(&self, id: FilterId) -> RpcResult<FilterChanges> {
        trace!(target: "rpc::eth", "Serving eth_getFilterLogs");
        Ok(EthFilter::filter_logs(self, id).await?)
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

impl<Provider, Pool> std::fmt::Debug for EthFilter<Provider, Pool> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EthFilter").finish_non_exhaustive()
    }
}

impl<Provider, Pool> Clone for EthFilter<Provider, Pool> {
    fn clone(&self) -> Self {
        Self { inner: Arc::clone(&self.inner) }
    }
}

/// Container type `EthFilter`
#[derive(Debug)]
struct EthFilterInner<Provider, Pool> {
    /// The transaction pool.
    pool: Pool,
    /// The provider that can interact with the chain.
    provider: Provider,
    /// All currently installed filters.
    active_filters: ActiveFilters,
    /// Provides ids to identify filters
    id_provider: Arc<dyn IdProvider>,
    /// Maximum number of blocks that could be scanned per filter
    max_blocks_per_filter: u64,
    /// Maximum number of logs that can be returned in a response
    max_logs_per_response: usize,
    /// The async cache frontend for eth related data
    eth_cache: EthStateCache,
    /// The type that can spawn tasks.
    task_spawner: Box<dyn TaskSpawner>,
    /// Duration since the last filter poll, after which the filter is considered stale
    stale_filter_ttl: Duration,
}

impl<Provider, Pool> EthFilterInner<Provider, Pool>
where
    Provider: BlockReader + BlockIdReader + EvmEnvProvider + LogHistoryReader + 'static,
    Pool: TransactionPool + 'static,
{
    /// Returns logs matching given filter object.
    async fn logs_for_filter(&self, filter: Filter) -> Result<Vec<Log>, FilterError> {
        match filter.block_option {
            FilterBlockOption::AtBlockHash(block_hash) => {
                let mut all_logs = Vec::new();
                // all matching logs in the block, if it exists
                if let Some((block, receipts)) =
                    self.eth_cache.get_block_and_receipts(block_hash).await?
                {
                    let filter = FilteredParams::new(Some(filter));
                    logs_utils::append_matching_block_logs(
                        &mut all_logs,
                        &filter,
                        (block_hash, block.number).into(),
                        block.body.into_iter().map(|tx| tx.hash()).zip(receipts),
                        false,
                    );
                }
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
                self.get_logs_in_block_range(&filter, from_block_number, to_block_number).await
            }
        }
    }

    /// Installs a new filter and returns the new identifier.
    async fn install_filter(&self, kind: FilterKind) -> RpcResult<FilterId> {
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

    /// Fetches both receipts and block for the given block number.
    async fn block_and_receipts_by_number(
        &self,
        hash_or_number: BlockHashOrNumber,
    ) -> EthResult<Option<(SealedBlock, Vec<Receipt>)>> {
        let block_hash = match self.provider.convert_block_hash(hash_or_number)? {
            Some(hash) => hash,
            None => return Ok(None),
        };

        Ok(self.eth_cache.get_block_and_receipts(block_hash).await?)
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
    ) -> Result<Vec<Log>, FilterError> {
        trace!(target: "rpc::eth::filter", from=from_block, to=to_block, ?filter, "finding logs in range");

        if to_block - from_block > self.max_blocks_per_filter {
            return Err(FilterError::QueryExceedsMaxBlocks(self.max_blocks_per_filter))
        }

        let mut all_logs = Vec::new();
        let filter_params = FilteredParams::new(Some(filter.clone()));

        // derive bloom filters from filter input
        // let address_filter = FilteredParams::address_filter(&filter.address);
        // let topics_filter = FilteredParams::topics_filter(&filter.topics);

        // Create log index filter
        let mut log_index_filter = LogIndexFilter::new(from_block..=to_block);
        if let Some(filter_address) = filter.address.to_value_or_array() {
            log_index_filter.install_address_filter(&self.provider, &filter_address)?;
        }
        let topics = filter
            .topics
            .clone()
            .into_iter()
            .filter_map(|t| t.to_value_or_array())
            .collect::<Vec<_>>();
        if !topics.is_empty() {
            log_index_filter.install_topic_filter(&self.provider, &topics)?;
        }

        let is_multi_block_range = from_block != to_block;

        // Since we know that there is at least one log per block, we can check whether the number
        // of blocks exceeds the max logs response.
        if let Some(Some(index)) = &log_index_filter.index {
            if is_multi_block_range && index.len() > self.max_logs_per_response {
                return Err(FilterError::QueryExceedsMaxResults(self.max_logs_per_response))
            }
        }

        // TODO: simplify
        // loop over the range of new blocks and check logs if the filter matches the log's bloom
        for block_number in log_index_filter.iter() {
            if let Some((block, receipts)) =
                self.block_and_receipts_by_number(block_number.into()).await?
            {
                logs_utils::append_matching_block_logs(
                    &mut all_logs,
                    &filter_params,
                    (block.number, block.hash).into(),
                    block.body.into_iter().map(|tx| tx.hash()).zip(receipts),
                    false,
                );

                // size check but only if range is multiple blocks, so we always return all
                // logs of a single block
                if is_multi_block_range && all_logs.len() > self.max_logs_per_response {
                    return Err(FilterError::QueryExceedsMaxResults(self.max_logs_per_response))
                }
            }
        }

        Ok(all_logs)
    }
}

/// Config for the filter
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EthFilterConfig {
    /// Maximum number of blocks that a filter can scan for logs.
    ///
    /// If `None` then no limit is enforced.
    pub max_blocks_per_filter: Option<u64>,
    /// Maximum number of logs that can be returned in a single response in `eth_getLogs` calls.
    ///
    /// If `None` then no limit is enforced.
    pub max_logs_per_response: Option<usize>,
    /// How long a filter remains valid after the last poll.
    ///
    /// A filter is considered stale if it has not been polled for longer than this duration and
    /// will be removed.
    pub stale_filter_ttl: Duration,
}

impl EthFilterConfig {
    /// Sets the maximum number of blocks that a filter can scan for logs.
    pub fn max_blocks_per_filter(mut self, num: u64) -> Self {
        self.max_blocks_per_filter = Some(num);
        self
    }

    /// Sets the maximum number of logs that can be returned in a single response in `eth_getLogs`
    /// calls.
    pub fn max_logs_per_response(mut self, num: usize) -> Self {
        self.max_logs_per_response = Some(num);
        self
    }

    /// Sets how long a filter remains valid after the last poll before it will be removed.
    pub fn stale_filter_ttl(mut self, duration: Duration) -> Self {
        self.stale_filter_ttl = duration;
        self
    }
}

impl Default for EthFilterConfig {
    fn default() -> Self {
        Self {
            max_blocks_per_filter: None,
            max_logs_per_response: None,
            // 5min
            stale_filter_ttl: Duration::from_secs(5 * 60),
        }
    }
}

type BlockNumberIterBox<'a> = Box<dyn Iterator<Item = BlockNumber> + Send + Sync + 'a>;

/// TODO: docs
#[derive(Debug)]
pub struct LogIndexFilter {
    /// Full block range.
    block_range: RangeInclusive<BlockNumber>,
    /// Represents the union of all block numbers that match installed filters.
    ///
    /// Since [IntegerList] cannot be empty, `Some(None)` represents an active filter with no
    /// matching block numbers.
    index: Option<Option<IntegerList>>,
}

impl LogIndexFilter {
    /// Create new instance of [LogIndexFilter].
    pub fn new(block_range: RangeInclusive<BlockNumber>) -> Self {
        Self { block_range, index: None }
    }

    /// Return the block range for this filter.
    pub fn block_range(&self) -> RangeInclusive<BlockNumber> {
        self.block_range.clone()
    }

    /// Iterate over block numbers.
    pub fn iter<'a>(&'a self) -> BlockNumberIterBox<'a> {
        match &self.index {
            Some(Some(indices)) => {
                Box::new(indices.iter(0).map(|num| num as BlockNumber)) as BlockNumberIterBox<'a>
            }
            Some(None) => Box::new(std::iter::empty()) as BlockNumberIterBox<'a>,
            None => Box::new(self.block_range()) as BlockNumberIterBox<'a>,
        }
    }

    /// Returns `true` if the filter is active but has no matching block numbers.
    pub fn is_empty(&self) -> bool {
        matches!(self.index, Some(None))
    }

    /// Change the inner indices to be the union of itself and incoming.
    pub fn index_intersection(&mut self, new_indices: Option<IntegerList>) {
        match (&mut self.index, new_indices) {
            // Existing filter is already empty, no need to update.
            (Some(None), _) => (),
            // Incoming filter matched no block numbers, reset existing indices.
            (_, None) => {
                self.index = Some(None);
            }
            // Existing filter has not been yet activated, so set it to the incoming indices.
            (None, Some(new_indices)) => {
                self.index = Some(Some(new_indices));
            }
            // Both filters contain at least one block number, union them.
            (Some(Some(existing)), Some(new_indices)) => {
                self.index = Some(existing.intersection(&new_indices));
            }
        }
    }

    /// Intersect current filter with the new address filter.
    pub fn install_address_filter(
        &mut self,
        provider: &impl LogHistoryReader,
        address_filter: &ValueOrArray<Address>,
    ) -> Result<(), FilterError> {
        // Check if previous filter conditions already matched no blocks.
        // Do this on each iteration to avoid unnecessary queries.
        if self.is_empty() {
            return Ok(())
        }

        let address_index = match address_filter {
            ValueOrArray::Value(address) => {
                provider.log_address_index(*address, self.block_range())?
            }
            ValueOrArray::Array(addresses) => {
                let mut address_position_union: Option<IntegerList> = None;
                for address in addresses {
                    let topic_index = provider.log_address_index(*address, self.block_range())?;
                    address_position_union = match (&address_position_union, &topic_index) {
                        (Some(list1), Some(list2)) => Some(list1.union(list2)),
                        _ => address_position_union.or(topic_index),
                    };
                }
                address_position_union
            }
        };
        self.index_intersection(address_index);

        Ok(())
    }

    /// Intersect current filter with the new topic filter.
    pub fn install_topic_filter(
        &mut self,
        provider: &impl LogHistoryReader,
        topic_filters: &[ValueOrArray<B256>],
    ) -> Result<(), FilterError> {
        for topic_filter in topic_filters {
            // Check if previous filter conditions already matched no blocks.
            // Do this on each iteration to avoid unnecessary queries.
            if self.is_empty() {
                return Ok(())
            }

            let topic_position_index = match topic_filter {
                ValueOrArray::Value(topic) => {
                    provider.log_topic_index(*topic, self.block_range())?
                }
                ValueOrArray::Array(topics) => {
                    let mut topic_position_union: Option<IntegerList> = None;
                    for topic in topics {
                        let topic_index = provider.log_topic_index(*topic, self.block_range())?;
                        topic_position_union = match (&topic_position_union, &topic_index) {
                            (Some(list1), Some(list2)) => Some(list1.union(list2)),
                            _ => topic_position_union.or(topic_index),
                        };
                    }
                    topic_position_union
                }
            };
            self.index_intersection(topic_position_index);
        }
        Ok(())
    }
}

/// All active filters
#[derive(Debug, Clone, Default)]
pub struct ActiveFilters {
    inner: Arc<Mutex<HashMap<FilterId, ActiveFilter>>>,
}

/// An installed filter
#[derive(Debug)]
struct ActiveFilter {
    /// At which block the filter was polled last.
    block: u64,
    /// Last time this filter was polled.
    last_poll_timestamp: Instant,
    /// What kind of filter it is.
    kind: FilterKind,
}

/// A receiver for pending transactions that returns all new transactions since the last poll.
#[derive(Debug, Clone)]
struct PendingTransactionsReceiver {
    txs_receiver: Arc<Mutex<Receiver<TxHash>>>,
}

impl PendingTransactionsReceiver {
    fn new(receiver: Receiver<TxHash>) -> Self {
        PendingTransactionsReceiver { txs_receiver: Arc::new(Mutex::new(receiver)) }
    }

    /// Returns all new pending transactions received since the last poll.
    async fn drain(&self) -> FilterChanges {
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
struct FullTransactionsReceiver<T: PoolTransaction> {
    txs_stream: Arc<Mutex<NewSubpoolTransactionStream<T>>>,
}

impl<T> FullTransactionsReceiver<T>
where
    T: PoolTransaction + 'static,
{
    /// Creates a new `FullTransactionsReceiver` encapsulating the provided transaction stream.
    fn new(stream: NewSubpoolTransactionStream<T>) -> Self {
        FullTransactionsReceiver { txs_stream: Arc::new(Mutex::new(stream)) }
    }

    /// Returns all new pending transactions received since the last poll.
    async fn drain(&self) -> FilterChanges {
        let mut pending_txs = Vec::new();
        let mut prepared_stream = self.txs_stream.lock().await;

        while let Ok(tx) = prepared_stream.try_recv() {
            pending_txs.push(reth_rpc_types_compat::transaction::from_recovered(
                tx.transaction.to_recovered_transaction(),
            ))
        }
        FilterChanges::Transactions(pending_txs)
    }
}

/// Helper trait for [FullTransactionsReceiver] to erase the `Transaction` type.
#[async_trait]
trait FullTransactionsFilter: fmt::Debug + Send + Sync + Unpin + 'static {
    async fn drain(&self) -> FilterChanges;
}

#[async_trait]
impl<T> FullTransactionsFilter for FullTransactionsReceiver<T>
where
    T: PoolTransaction + 'static,
{
    async fn drain(&self) -> FilterChanges {
        FullTransactionsReceiver::drain(self).await
    }
}

/// Represents the kind of pending transaction data that can be retrieved.
///
/// This enum differentiates between two kinds of pending transaction data:
/// - Just the transaction hashes.
/// - Full transaction details.
#[derive(Debug, Clone)]
enum PendingTransactionKind {
    Hashes(PendingTransactionsReceiver),
    FullTransaction(Arc<dyn FullTransactionsFilter>),
}

impl PendingTransactionKind {
    async fn drain(&self) -> FilterChanges {
        match self {
            PendingTransactionKind::Hashes(receiver) => receiver.drain().await,
            PendingTransactionKind::FullTransaction(receiver) => receiver.drain().await,
        }
    }
}

#[derive(Clone, Debug)]
enum FilterKind {
    Log(Box<Filter>),
    Block,
    PendingTransaction(PendingTransactionKind),
}
/// Errors that can occur in the handler implementation
#[derive(Debug, thiserror::Error)]
pub enum FilterError {
    /// Requested filter was not found.
    #[error("filter not found")]
    FilterNotFound(FilterId),
    /// Query exceeds maximum allowed blocks per response.
    #[error("query exceeds max block range {0}")]
    QueryExceedsMaxBlocks(u64),
    /// Query exceeds maximum allowed results per response.
    #[error("query exceeds max results {0}")]
    QueryExceedsMaxResults(usize),
    /// Wrapper around [EthApiError].
    #[error(transparent)]
    EthAPIError(#[from] EthApiError),
    /// Error thrown when a spawned task failed to deliver a response.
    #[error("internal filter error")]
    InternalError,
}

// convert the error
impl From<FilterError> for jsonrpsee::types::error::ErrorObject<'static> {
    fn from(err: FilterError) -> Self {
        match err {
            FilterError::FilterNotFound(_) => rpc_error_with_code(
                jsonrpsee::types::error::INVALID_PARAMS_CODE,
                "filter not found",
            ),
            err @ FilterError::InternalError => {
                rpc_error_with_code(jsonrpsee::types::error::INTERNAL_ERROR_CODE, err.to_string())
            }
            FilterError::EthAPIError(err) => err.into(),
            err @ FilterError::QueryExceedsMaxBlocks(_) => {
                rpc_error_with_code(jsonrpsee::types::error::INVALID_PARAMS_CODE, err.to_string())
            }
            err @ FilterError::QueryExceedsMaxResults(_) => {
                rpc_error_with_code(jsonrpsee::types::error::INVALID_PARAMS_CODE, err.to_string())
            }
        }
    }
}

impl From<ProviderError> for FilterError {
    fn from(err: ProviderError) -> Self {
        FilterError::EthAPIError(err.into())
    }
}
