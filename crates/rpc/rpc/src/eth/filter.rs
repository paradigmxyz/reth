use super::cache::EthStateCache;
use crate::{
    eth::{
        error::{EthApiError, EthResult},
        logs_utils,
    },
    result::{internal_rpc_err, rpc_error_with_code, ToRpcResult},
    EthSubscriptionIdProvider,
};
use async_trait::async_trait;
use jsonrpsee::{core::RpcResult, server::IdProvider};
use reth_primitives::{
    filter::{Filter, FilterBlockOption, FilteredParams},
    SealedBlock,
};
use reth_provider::{BlockProvider, EvmEnvProvider};
use reth_rpc_api::EthFilterApiServer;
use reth_rpc_types::{FilterChanges, FilterId, Log};
use reth_transaction_pool::TransactionPool;
use std::{collections::HashMap, sync::Arc, time::Instant};
use tokio::sync::Mutex;
use tracing::trace;

/// `Eth` filter RPC implementation.
#[derive(Debug, Clone)]
pub struct EthFilter<Client, Pool> {
    /// All nested fields bundled together.
    inner: Arc<EthFilterInner<Client, Pool>>,
}

impl<Client, Pool> EthFilter<Client, Pool> {
    /// Creates a new, shareable instance.
    ///
    /// This uses the given pool to get notified about new transactions, the client to interact with
    /// the blockchain, the cache to fetch cacheable data, like the logs and the
    /// max_logs_per_response to limit the amount of logs returned in a single response
    /// `eth_getLogs`
    pub fn new(
        client: Client,
        pool: Pool,
        eth_cache: EthStateCache,
        max_logs_per_response: usize,
    ) -> Self {
        let inner = EthFilterInner {
            client,
            active_filters: Default::default(),
            pool,
            id_provider: Arc::new(EthSubscriptionIdProvider::default()),
            max_logs_per_response,
            eth_cache,
        };
        Self { inner: Arc::new(inner) }
    }

    /// Returns all currently active filters
    pub fn active_filters(&self) -> &ActiveFilters {
        &self.inner.active_filters
    }
}

#[async_trait]
impl<Client, Pool> EthFilterApiServer for EthFilter<Client, Pool>
where
    Client: BlockProvider + EvmEnvProvider + 'static,
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
    async fn new_pending_transaction_filter(&self) -> RpcResult<FilterId> {
        trace!(target: "rpc::eth", "Serving eth_newPendingTransactionFilter");
        self.inner.install_filter(FilterKind::PendingTransaction).await
    }

    /// Handler for `eth_getFilterChanges`
    async fn filter_changes(&self, id: FilterId) -> RpcResult<FilterChanges> {
        trace!(target: "rpc::eth", "Serving eth_getFilterChanges");
        let info = self.inner.client.chain_info().to_rpc_result()?;
        let best_number = info.best_number;

        let (start_block, kind) = {
            let mut filters = self.inner.active_filters.inner.lock().await;
            let filter = filters.get_mut(&id).ok_or(FilterError::FilterNotFound(id))?;

            // update filter
            // we fetch all changes from [filter.block..best_block], so we advance the filter's
            // block to `best_block +1`
            let mut block = best_number + 1;
            std::mem::swap(&mut filter.block, &mut block);
            filter.last_poll_timestamp = Instant::now();

            (block, filter.kind.clone())
        };

        match kind {
            FilterKind::PendingTransaction => {
                return Err(internal_rpc_err("method not implemented"))
            }
            FilterKind::Block => {
                let mut block_hashes = Vec::new();
                for block_num in start_block..best_number {
                    let block_hash = self
                        .inner
                        .client
                        .block_hash(block_num)
                        .to_rpc_result()?
                        .ok_or(EthApiError::UnknownBlockNumber)?;
                    block_hashes.push(block_hash);
                }
                Ok(FilterChanges::Hashes(block_hashes))
            }
            FilterKind::Log(filter) => {
                let (from_block_number, to_block_number) = match filter.block_option {
                    FilterBlockOption::Range { from_block, to_block } => {
                        logs_utils::get_filter_block_range(from_block, to_block, start_block, info)
                    }
                    FilterBlockOption::AtBlockHash(_) => {
                        // blockHash is equivalent to fromBlock = toBlock = the block number with
                        // hash blockHash
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
    async fn filter_logs(&self, id: FilterId) -> RpcResult<Vec<Log>> {
        trace!(target: "rpc::eth", "Serving eth_getFilterLogs");
        let filter = {
            let filters = self.inner.active_filters.inner.lock().await;
            if let FilterKind::Log(ref filter) =
                filters.get(&id).ok_or_else(|| FilterError::FilterNotFound(id.clone()))?.kind
            {
                *filter.clone()
            } else {
                // Not a log filter
                return Err(FilterError::FilterNotFound(id).into())
            }
        };

        self.inner.logs_for_filter(filter).await
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
        self.inner.logs_for_filter(filter).await
    }
}

/// Container type `EthFilter`
#[derive(Debug)]
struct EthFilterInner<Client, Pool> {
    /// The transaction pool.
    pool: Pool,
    /// The client that can interact with the chain.
    client: Client,
    /// All currently installed filters.
    active_filters: ActiveFilters,
    /// Provides ids to identify filters
    id_provider: Arc<dyn IdProvider>,
    /// Maximum number of logs that can be returned in a response
    max_logs_per_response: usize,
    /// The async cache frontend for eth related data
    eth_cache: EthStateCache,
}

impl<Client, Pool> EthFilterInner<Client, Pool>
where
    Client: BlockProvider + EvmEnvProvider + 'static,
    Pool: TransactionPool + 'static,
{
    /// Returns logs matching given filter object.
    async fn logs_for_filter(&self, filter: Filter) -> RpcResult<Vec<Log>> {
        match filter.block_option {
            FilterBlockOption::AtBlockHash(block_hash) => {
                let mut all_logs = Vec::new();
                // all matching logs in the block, if it exists
                if let Some(block) = self.eth_cache.get_block(block_hash).await.to_rpc_result()? {
                    // get receipts for the block
                    if let Some(receipts) =
                        self.eth_cache.get_receipts(block_hash).await.to_rpc_result()?
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
                }
                Ok(all_logs)
            }
            FilterBlockOption::Range { from_block, to_block } => {
                // compute the range
                let info = self.client.chain_info().to_rpc_result()?;

                // we start at the most recent block if unset in filter
                let start_block = info.best_number;
                let (from_block_number, to_block_number) =
                    logs_utils::get_filter_block_range(from_block, to_block, start_block, info);
                Ok(self
                    .get_logs_in_block_range(&filter, from_block_number, to_block_number)
                    .await?)
            }
        }
    }

    /// Installs a new filter and returns the new identifier.
    async fn install_filter(&self, kind: FilterKind) -> RpcResult<FilterId> {
        let last_poll_block_number = self.client.best_block_number().to_rpc_result()?;
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

    /// Returns the block with the given block number if it exists.
    async fn block_by_number(&self, num: u64) -> EthResult<Option<SealedBlock>> {
        match self.client.block_hash(num)? {
            Some(hash) => Ok(self.eth_cache.get_sealed_block(hash).await?),
            None => Ok(None),
        }
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

        let mut all_logs = Vec::new();
        let filter_params = FilteredParams::new(Some(filter.clone()));

        let topics = filter.has_topics().then(|| filter_params.flat_topics.clone());

        // derive bloom filters from filter input
        let address_filter = FilteredParams::address_filter(&filter.address);
        let topics_filter = FilteredParams::topics_filter(&topics);

        // loop over the range of new blocks and check logs if the filter matches the log's bloom
        // filter
        for block_number in from_block..=to_block {
            if let Some(block) = self.block_by_number(block_number).await? {
                // only if filter matches
                if FilteredParams::matches_address(block.header.logs_bloom, &address_filter) &&
                    FilteredParams::matches_topics(block.header.logs_bloom, &topics_filter)
                {
                    // get receipts for the block
                    if let Some(receipts) = self.eth_cache.get_receipts(block.hash).await? {
                        let block_hash = block.hash;

                        logs_utils::append_matching_block_logs(
                            &mut all_logs,
                            &filter_params,
                            (block_number, block_hash).into(),
                            block.body.into_iter().map(|tx| tx.hash()).zip(receipts),
                            false,
                        );

                        // size check
                        if all_logs.len() > self.max_logs_per_response {
                            return Err(FilterError::QueryExceedsMaxResults(
                                self.max_logs_per_response,
                            ))
                        }
                    }
                }
            }
        }

        Ok(all_logs)
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

#[derive(Clone, Debug)]
enum FilterKind {
    Log(Box<Filter>),
    Block,
    PendingTransaction,
}

/// Errors that can occur in the handler implementation
#[derive(Debug, thiserror::Error)]
pub enum FilterError {
    #[error("filter not found")]
    FilterNotFound(FilterId),
    #[error("Query exceeds max results {0}")]
    QueryExceedsMaxResults(usize),
    #[error(transparent)]
    EthAPIError(#[from] EthApiError),
}

// convert the error
impl From<FilterError> for jsonrpsee::core::Error {
    fn from(err: FilterError) -> Self {
        match err {
            FilterError::FilterNotFound(_) => rpc_error_with_code(
                jsonrpsee::types::error::INVALID_PARAMS_CODE,
                "filter not found",
            ),
            FilterError::EthAPIError(err) => err.into(),
            err @ FilterError::QueryExceedsMaxResults(_) => {
                rpc_error_with_code(jsonrpsee::types::error::INVALID_PARAMS_CODE, err.to_string())
            }
        }
    }
}

impl From<reth_interfaces::Error> for FilterError {
    fn from(err: reth_interfaces::Error) -> Self {
        FilterError::EthAPIError(err.into())
    }
}
