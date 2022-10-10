use jsonrpsee::{core::RpcResult as Result, proc_macros::rpc};
use reth_primitives::{rpc::Filter, U256};
use reth_rpc_types::{FilterChanges, Index, Log};

/// Rpc Interface for poll-based ethereum filter API.
#[cfg_attr(not(feature = "client"), rpc(server))]
#[cfg_attr(feature = "client", rpc(server, client))]
pub trait EthFilterApi {
    /// Creates anew filter and returns its id.
    #[method(name = "eth_newFilter")]
    fn new_filter(&self, filter: Filter) -> Result<U256>;

    /// Creates a new block filter and returns its id.
    #[method(name = "eth_newBlockFilter")]
    fn new_block_filter(&self) -> Result<U256>;

    /// Creates a pending transaction filter and returns its id.
    #[method(name = "eth_newPendingTransactionFilter")]
    fn new_pending_transaction_filter(&self) -> Result<U256>;

    /// Returns all filter changes since last poll.
    #[method(name = "eth_getFilterChanges")]
    async fn filter_changes(&self, index: Index) -> Result<FilterChanges>;

    /// Returns all logs matching given filter (in a range 'from' - 'to').
    #[method(name = "eth_getFilterLogs")]
    async fn filter_logs(&self, index: Index) -> Result<Vec<Log>>;

    /// Uninstalls filter.
    #[method(name = "eth_uninstallFilter")]
    fn uninstall_filter(&self, index: Index) -> Result<bool>;

    /// Returns logs matching given filter object.
    #[method(name = "eth_getLogs")]
    async fn logs(&self, filter: Filter) -> Result<Vec<Log>>;
}
