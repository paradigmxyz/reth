use crate::{
    eth::{error::EthApiError, logs_utils},
    result::{internal_rpc_err, rpc_error_with_code, ToRpcResult},
    EthSubscriptionIdProvider,
};
use async_trait::async_trait;
use jsonrpsee::{core::RpcResult, server::IdProvider};
use reth_primitives::{
    filter::{Filter, FilterBlockOption, FilteredParams},
    U256,
};
use reth_provider::{BlockProvider, EvmEnvProvider};
use reth_rpc_api::EthFilterApiServer;
use reth_rpc_types::{FilterChanges, FilterId, Log};
use reth_transaction_pool::TransactionPool;
use std::{collections::HashMap, sync::Arc, time::Instant};
use tokio::sync::Mutex;
use tracing::trace;

/// The default maximum of logs in a single response.
const DEFAULT_MAX_LOGS_IN_RESPONSE: usize = 2_000;

/// `Eth` filter RPC implementation.
#[derive(Debug, Clone)]
pub struct EthFilter<Client, Pool> {
    /// All nested fields bundled together.
    inner: Arc<EthFilterInner<Client, Pool>>,
}

impl<Client, Pool> EthFilter<Client, Pool> {
    /// Creates a new, shareable instance.
    pub fn new(client: Client, pool: Pool) -> Self {
        let inner = EthFilterInner {
            client,
            active_filters: Default::default(),
            pool,
            id_provider: Arc::new(EthSubscriptionIdProvider::default()),
            max_logs_in_response: DEFAULT_MAX_LOGS_IN_RESPONSE,
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
        self.inner.install_filter(FilterKind::Log(Box::new(filter))).await
    }

    /// Handler for `eth_newBlockFilter`
    async fn new_block_filter(&self) -> RpcResult<FilterId> {
        self.inner.install_filter(FilterKind::Block).await
    }

    /// Handler for `eth_newPendingTransactionFilter`
    async fn new_pending_transaction_filter(&self) -> RpcResult<FilterId> {
        self.inner.install_filter(FilterKind::PendingTransaction).await
    }

    /// Handler for `eth_getFilterChanges`
    async fn filter_changes(&self, id: FilterId) -> RpcResult<FilterChanges> {
        let info = self.inner.client.chain_info().to_rpc_result()?;
        let best_number = info.best_number;

        let (start_block, kind) = {
            let mut filters = self.inner.active_filters.inner.lock().await;
            let mut filter = filters.get_mut(&id).ok_or(FilterError::FilterNotFound(id))?;

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
                        .block_hash(U256::from(block_num))
                        .to_rpc_result()?
                        .ok_or(EthApiError::UnknownBlockNumber)?;
                    block_hashes.push(block_hash);
                }
                Ok(FilterChanges::Hashes(block_hashes))
            }
            FilterKind::Log(filter) => {
                let mut from_block_number = start_block;
                let mut to_block_number = best_number;
                match filter.block_option {
                    FilterBlockOption::Range { from_block, to_block } => {
                        // from block is maximum of block from last poll or `from_block` of filter
                        if let Some(filter_from_block) =
                            from_block.and_then(|num| info.convert_block_number(num.into()))
                        {
                            from_block_number = start_block.max(filter_from_block)
                        }

                        // to block is max the best number
                        if let Some(filter_to_block) =
                            to_block.and_then(|num| info.convert_block_number(num.into()))
                        {
                            to_block_number = filter_to_block;
                            if to_block_number > best_number {
                                to_block_number = best_number;
                            }
                        }
                    }
                    FilterBlockOption::AtBlockHash(_) => {
                        // blockHash is equivalent to fromBlock = toBlock = the block number with
                        // hash blockHash
                    }
                }

                self.inner
                    .filter_logs(&filter, from_block_number, to_block_number)
                    .map(FilterChanges::Logs)
            }
        }
    }

    /// Handler for `eth_getFilterLogs`
    async fn filter_logs(&self, _id: FilterId) -> RpcResult<Vec<Log>> {
        todo!()
    }

    /// Handler for `eth_uninstallFilter`
    async fn uninstall_filter(&self, id: FilterId) -> RpcResult<bool> {
        let mut filters = self.inner.active_filters.inner.lock().await;
        if filters.remove(&id).is_some() {
            trace!(target: "rpc::eth::filter", ?id, "uninstalled filter");
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Handler for `eth_getLogs`
    async fn logs(&self, _filter: Filter) -> RpcResult<Vec<Log>> {
        todo!()
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
    max_logs_in_response: usize,
}

impl<Client, Pool> EthFilterInner<Client, Pool>
where
    Client: BlockProvider + EvmEnvProvider + 'static,
    Pool: TransactionPool + 'static,
{
    /// Installs a new filter and returns the new identifier.
    async fn install_filter(&self, kind: FilterKind) -> RpcResult<FilterId> {
        let last_poll_block_number = self.client.chain_info().to_rpc_result()?.best_number;
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

    /// Returns all logs in the given range that match the filter
    ///
    /// Returns an error if:
    ///  - underlying database error
    ///  - amount of matches exceeds configured limit
    fn filter_logs(&self, filter: &Filter, from_block: u64, to_block: u64) -> RpcResult<Vec<Log>> {
        let mut all_logs = Vec::new();
        let filter_params = FilteredParams::new(Some(filter.clone()));

        let topics =
            if filter.has_topics() { Some(filter_params.flat_topics.clone()) } else { None };

        // derive bloom filters from filter input
        let address_filter = FilteredParams::address_filter(&filter.address);
        let topics_filter = FilteredParams::topics_filter(&topics);

        // loop over the range of new blocks and check logs if the filter matches the log's bloom
        // filter
        for block_number in from_block..=to_block {
            if let Some(block) = self.client.block_by_number(block_number).to_rpc_result()? {
                // only if filter matches
                if FilteredParams::matches_address(block.header.logs_bloom, &address_filter) &&
                    FilteredParams::matches_topics(block.header.logs_bloom, &topics_filter)
                {
                    // get receipts for the block
                    if let Some(receipts) =
                        self.client.receipts_by_block(block.number.into()).to_rpc_result()?
                    {
                        let block_hash = block.hash_slow();

                        logs_utils::append_matching_block_logs(
                            &mut all_logs,
                            &filter_params,
                            block_hash,
                            block_number,
                            block.body.into_iter().map(|tx| tx.hash).zip(receipts),
                        );

                        // size check
                        if all_logs.len() > self.max_logs_in_response {
                            return Err(FilterError::QueryExceedsMaxResults(
                                self.max_logs_in_response,
                            )
                            .into())
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
#[derive(Debug, Clone, thiserror::Error)]
pub enum FilterError {
    #[error("filter not found")]
    FilterNotFound(FilterId),
    #[error("Query exceeds max results {0}")]
    QueryExceedsMaxResults(usize),
}

// convert the error
impl From<FilterError> for jsonrpsee::core::Error {
    fn from(err: FilterError) -> Self {
        match err {
            FilterError::FilterNotFound(_) => rpc_error_with_code(
                jsonrpsee::types::error::INVALID_PARAMS_CODE,
                "filter not found",
            ),
            err @ FilterError::QueryExceedsMaxResults(_) => {
                rpc_error_with_code(jsonrpsee::types::error::INVALID_PARAMS_CODE, err.to_string())
            }
        }
    }
}
