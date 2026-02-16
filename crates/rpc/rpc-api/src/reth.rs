use alloy_eips::BlockId;
use alloy_primitives::{map::AddressMap, U256};
use alloy_rpc_types_engine::PayloadStatus;
use jsonrpsee::{core::RpcResult, proc_macros::rpc};
use reth_engine_primitives::ExecutionPayload;
use serde::{Deserialize, Serialize};

// Required for the subscription attributes below
use reth_chain_state as _;

/// Reth-specific payload status that includes server-measured execution latency.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RethPayloadStatus {
    /// The standard payload status.
    #[serde(flatten)]
    pub status: PayloadStatus,
    /// Server-side execution latency in microseconds.
    pub latency_us: u64,
    /// Time spent waiting for persistence to complete, in microseconds.
    /// `None` when no persistence was in-flight.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub persistence_wait_us: Option<u64>,
    /// Time spent waiting for the execution cache lock, in microseconds.
    pub execution_cache_wait_us: u64,
    /// Time spent waiting for the sparse trie lock, in microseconds.
    pub sparse_trie_wait_us: u64,
}

/// Reth API namespace for reth-specific methods
#[cfg_attr(not(feature = "client"), rpc(server, namespace = "reth"), server_bounds(Payload: jsonrpsee::core::DeserializeOwned))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "reth", client_bounds(Payload: jsonrpsee::core::Serialize + Clone), server_bounds(Payload: jsonrpsee::core::DeserializeOwned)))]
pub trait RethApi<Payload: ExecutionPayload> {
    /// Reth-specific newPayload that takes execution data directly.
    ///
    /// Waits for persistence, execution cache, and sparse trie locks before processing.
    #[method(name = "newPayload")]
    async fn reth_new_payload(&self, payload: Payload) -> RpcResult<RethPayloadStatus>;

    /// Returns all ETH balance changes in a block
    #[method(name = "getBalanceChangesInBlock")]
    async fn reth_get_balance_changes_in_block(
        &self,
        block_id: BlockId,
    ) -> RpcResult<AddressMap<U256>>;

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
