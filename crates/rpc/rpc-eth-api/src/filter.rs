//! `eth_` RPC API for filtering.

use alloy_json_rpc::RpcObject;
use alloy_rpc_types_eth::{Filter, FilterChanges, FilterId, Log, PendingTransactionFilterKind};
use jsonrpsee::{core::RpcResult, proc_macros::rpc};
use reth_rpc_eth_types::LogsWithCursor;
use std::future::Future;

/// Rpc Interface for poll-based ethereum filter API.
#[cfg_attr(not(feature = "client"), rpc(server, namespace = "eth"))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "eth"))]
pub trait EthFilterApi<T: RpcObject> {
    /// Creates a new filter and returns its id.
    #[method(name = "newFilter")]
    async fn new_filter(&self, filter: Filter) -> RpcResult<FilterId>;

    /// Creates a new block filter and returns its id.
    #[method(name = "newBlockFilter")]
    async fn new_block_filter(&self) -> RpcResult<FilterId>;

    /// Creates a pending transaction filter and returns its id.
    #[method(name = "newPendingTransactionFilter")]
    async fn new_pending_transaction_filter(
        &self,
        kind: Option<PendingTransactionFilterKind>,
    ) -> RpcResult<FilterId>;

    /// Returns all filter changes since last poll.
    #[method(name = "getFilterChanges")]
    async fn filter_changes(&self, id: FilterId) -> RpcResult<FilterChanges<T>>;

    /// Returns all logs matching given filter (in a range 'from' - 'to').
    #[method(name = "getFilterLogs")]
    async fn filter_logs(&self, id: FilterId) -> RpcResult<Vec<Log>>;

    /// Uninstalls filter.
    #[method(name = "uninstallFilter")]
    async fn uninstall_filter(&self, id: FilterId) -> RpcResult<bool>;

    /// Returns logs matching given filter object.
    #[method(name = "getLogs")]
    async fn logs(&self, filter: Filter) -> RpcResult<Vec<Log>>;

    /// Returns logs matching given filter object with cursor-based pagination.
    #[method(name = "getLogsWithCursor")]
    async fn logs_with_cursor(
        &self,
        filter: Filter,
        cursor: Option<String>,
    ) -> RpcResult<LogsWithCursor>;
}

/// Limits for logs queries
#[derive(Debug, Clone, Copy)]
pub struct QueryLimits {
    /// Maximum number of blocks that could be scanned per filter
    pub max_blocks_per_filter: Option<u64>,
    /// Maximum number of logs that can be returned in a response
    pub max_logs_per_response: Option<usize>,
    /// Page size for cursor-based pagination (default: `10_000`)
    pub cursor_page_size: usize,
}

impl Default for QueryLimits {
    fn default() -> Self {
        Self { max_blocks_per_filter: None, max_logs_per_response: None, cursor_page_size: 10_000 }
    }
}

impl QueryLimits {
    /// Construct an object with no limits (more explicit than using default constructor)
    pub fn no_limits() -> Self {
        Default::default()
    }
}

/// Rpc Interface for poll-based ethereum filter API, implementing only the `eth_getLogs` method.
/// Used for the engine API, with possibility to specify [`QueryLimits`].
pub trait EngineEthFilter: Send + Sync + 'static {
    /// Returns logs matching given filter object.
    fn logs(
        &self,
        filter: Filter,
        limits: QueryLimits,
    ) -> impl Future<Output = RpcResult<Vec<Log>>> + Send;
}
