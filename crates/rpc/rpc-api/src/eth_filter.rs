use jsonrpsee::{core::RpcResult as Result, proc_macros::rpc, types::SubscriptionId};
use reth_primitives::rpc::Filter;
use reth_rpc_types::{FilterChanges, Log};

/// Rpc Interface for poll-based ethereum filter API.
#[cfg_attr(not(feature = "client"), rpc(server))]
#[cfg_attr(feature = "client", rpc(server))] // TODO(mattsse) make it work with SubscriptionId lifetime
pub trait EthFilterApi {
    /// Creates anew filter and returns its id.
    #[method(name = "eth_newFilter")]
    async fn new_filter(&self, filter: Filter) -> Result<SubscriptionId<'static>>;

    /// Creates a new block filter and returns its id.
    #[method(name = "eth_newBlockFilter")]
    async fn new_block_filter(&self) -> Result<SubscriptionId<'static>>;

    /// Creates a pending transaction filter and returns its id.
    #[method(name = "eth_newPendingTransactionFilter")]
    async fn new_pending_transaction_filter(&self) -> Result<SubscriptionId<'static>>;

    /// Returns all filter changes since last poll.
    #[method(name = "eth_getFilterChanges")]
    async fn filter_changes(&self, id: SubscriptionId<'_>) -> Result<FilterChanges>;

    /// Returns all logs matching given filter (in a range 'from' - 'to').
    #[method(name = "eth_getFilterLogs")]
    async fn filter_logs(&self, id: SubscriptionId<'_>) -> Result<Vec<Log>>;

    /// Uninstalls filter.
    #[method(name = "eth_uninstallFilter")]
    async fn uninstall_filter(&self, id: SubscriptionId<'_>) -> Result<bool>;

    /// Returns logs matching given filter object.
    #[method(name = "eth_getLogs")]
    async fn logs(&self, filter: Filter) -> Result<Vec<Log>>;
}
