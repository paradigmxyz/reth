//! `eth_` RPC API for filtering.

use alloy_json_rpc::RpcObject;
use alloy_rpc_types_eth::{Filter, FilterChanges, FilterId, PendingTransactionFilterKind};
use jsonrpsee::{core::RpcResult, proc_macros::rpc};
use reth_rpc_eth_types::logs_v2::{LogsV2Filter, LogsV2Result};
use std::future::Future;

/// Rpc Interface for poll-based ethereum filter API.
#[cfg_attr(not(feature = "client"), rpc(server, namespace = "eth"))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "eth"))]
pub trait EthFilterApi<T: RpcObject, L: RpcObject> {
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
    async fn filter_changes(&self, id: FilterId) -> RpcResult<FilterChanges<T, L>>;

    /// Returns all logs matching given filter (in a range 'from' - 'to').
    #[method(name = "getFilterLogs")]
    async fn filter_logs(&self, id: FilterId) -> RpcResult<Vec<L>>;

    /// Uninstalls filter.
    #[method(name = "uninstallFilter")]
    async fn uninstall_filter(&self, id: FilterId) -> RpcResult<bool>;

    /// Returns logs matching given filter object.
    #[method(name = "getLogs")]
    async fn logs(&self, filter: Filter) -> RpcResult<Vec<L>>;

    /// Returns logs matching the filter, one page at a time.
    ///
    /// The scan starts at the `from` block, or after the `from` cursor, which must be on the
    /// canonical chain. The response's `nextCursor` resumes the scan, possibly inside a block.
    ///
    /// Prototype based on <https://github.com/ethereum/execution-apis/pull/900>.
    #[method(name = "getLogsV2")]
    async fn logs_v2(&self, filter: LogsV2Filter) -> RpcResult<LogsV2Result>;
}

/// Limits for logs queries
#[derive(Default, Debug, Clone, Copy)]
pub struct QueryLimits {
    /// Maximum number of blocks that could be scanned per filter
    pub max_blocks_per_filter: Option<u64>,
    /// Maximum number of logs that can be returned in a response
    pub max_logs_per_response: Option<usize>,
}

impl QueryLimits {
    /// Construct an object with no limits (more explicit than using default constructor)
    pub fn no_limits() -> Self {
        Default::default()
    }
}

/// Rpc Interface for poll-based ethereum filter API, implementing only the `eth_getLogs` method.
/// Used for the engine API, with possibility to specify [`QueryLimits`].
pub trait EngineEthFilter<L: RpcObject>: Send + Sync + 'static {
    /// Returns logs matching given filter object.
    fn logs(
        &self,
        filter: Filter,
        limits: QueryLimits,
    ) -> impl Future<Output = RpcResult<Vec<L>>> + Send;
}
