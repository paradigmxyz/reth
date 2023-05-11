use jsonrpsee::{core::RpcResult, proc_macros::rpc};
use reth_primitives::filter::Filter;
use reth_rpc_types::{FilterChanges, FilterId, Log};

/// Rpc Interface for poll-based ethereum filter API.
#[cfg_attr(not(feature = "client"), rpc(server))]
#[cfg_attr(feature = "client", rpc(server, client))]
pub trait EthFilterApi {
    /// Creates anew filter and returns its id.
    #[method(name = "eth_newFilter")]
    async fn new_filter(&self, filter: Filter) -> RpcResult<FilterId>;

    /// Creates a new block filter and returns its id.
    #[method(name = "eth_newBlockFilter")]
    async fn new_block_filter(&self) -> RpcResult<FilterId>;

    /// Creates a pending transaction filter and returns its id.
    #[method(name = "eth_newPendingTransactionFilter")]
    async fn new_pending_transaction_filter(&self) -> RpcResult<FilterId>;

    /// Returns all filter changes since last poll.
    #[method(name = "eth_getFilterChanges")]
    async fn filter_changes(&self, id: FilterId) -> RpcResult<FilterChanges>;

    /// Returns all logs matching given filter (in a range 'from' - 'to').
    #[method(name = "eth_getFilterLogs")]
    async fn filter_logs(&self, id: FilterId) -> RpcResult<Vec<Log>>;

    /// Uninstalls filter.
    #[method(name = "eth_uninstallFilter")]
    async fn uninstall_filter(&self, id: FilterId) -> RpcResult<bool>;

    /// Returns logs matching given filter object.
    #[method(name = "eth_getLogs")]
    async fn logs(&self, filter: Filter) -> RpcResult<Vec<Log>>;
}
