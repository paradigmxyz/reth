use alloy_eips::BlockId;
use alloy_primitives::{map::AddressMap, U256, U64};
use jsonrpsee::{core::RpcResult, proc_macros::rpc};
use reth_execution_types::ExecutionOutcome;

// Required for the subscription attributes below
use reth_chain_state as _;

/// Reth API namespace for reth-specific methods
#[cfg_attr(not(feature = "client"), rpc(server, namespace = "reth"))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "reth"))]
pub trait RethApi {
    /// Returns all ETH balance changes in a block
    #[method(name = "getBalanceChangesInBlock")]
    async fn reth_get_balance_changes_in_block(
        &self,
        block_id: BlockId,
    ) -> RpcResult<AddressMap<U256>>;

    /// Re-executes a block (or a range of blocks) and returns the execution outcome including
    /// receipts, state changes, and EIP-7685 requests.
    ///
    /// If `count` is provided, re-executes `count` consecutive blocks starting from `block_id`
    /// and returns the merged execution outcome.
    #[method(name = "getBlockExecutionOutcome")]
    async fn reth_get_block_execution_outcome(
        &self,
        block_id: BlockId,
        count: Option<U64>,
    ) -> RpcResult<Option<ExecutionOutcome>>;

    /// Subscribe to json `ChainNotifications`
    #[subscription(
        name = "subscribeChainNotifications",
        unsubscribe = "unsubscribeChainNotifications",
        item = reth_chain_state::CanonStateNotification
    )]
    async fn reth_subscribe_chain_notifications(&self) -> jsonrpsee::core::SubscriptionResult;

    /// Subscribe to persisted block notifications.
    ///
    /// Emits a notification with the block number and hash when a new block is persisted to disk.
    #[subscription(
        name = "subscribePersistedBlock",
        unsubscribe = "unsubscribePersistedBlock",
        item = alloy_eips::BlockNumHash
    )]
    async fn reth_subscribe_persisted_block(&self) -> jsonrpsee::core::SubscriptionResult;

    /// Subscribe to finalized chain notifications.
    ///
    /// Buffers committed chain notifications and emits them once a new finalized block is received.
    /// Each notification contains all committed chain segments up to the finalized block.
    #[subscription(
        name = "subscribeFinalizedChainNotifications",
        unsubscribe = "unsubscribeFinalizedChainNotifications",
        item = Vec<reth_chain_state::CanonStateNotification>
    )]
    async fn reth_subscribe_finalized_chain_notifications(
        &self,
    ) -> jsonrpsee::core::SubscriptionResult;
}
