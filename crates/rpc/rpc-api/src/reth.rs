use alloy_eips::{eip7685::Requests, BlockId};
use alloy_primitives::{map::AddressMap, Address, Bytes, B256, U256};
use jsonrpsee::{core::RpcResult, proc_macros::rpc};
use std::collections::HashMap;

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

    /// Re-executes a block and returns the execution outcome including receipts, state changes,
    /// and EIP-7685 requests.
    #[method(name = "getBlockExecutionOutcome")]
    async fn reth_get_block_execution_outcome(
        &self,
        block_id: BlockId,
    ) -> RpcResult<Option<BlockExecutionOutcomeResponse>>;

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

/// Response type for `reth_getBlockExecutionOutcome`.
///
/// Contains the execution result of re-executing a block against its parent state.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockExecutionOutcomeResponse {
    /// Gas used by the block.
    pub gas_used: u64,
    /// Blob gas used by the block.
    pub blob_gas_used: u64,
    /// The receipts of the transactions in the block (serialized).
    pub receipts: Vec<serde_json::Value>,
    /// EIP-7685 requests.
    pub requests: Requests,
    /// State changes produced by the block execution, keyed by address.
    pub state_changes: HashMap<Address, AccountStateChanges>,
    /// New contract bytecodes deployed during block execution, keyed by code hash.
    pub contracts: HashMap<B256, Bytes>,
}

/// State changes for a single account.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountStateChanges {
    /// The account nonce after execution, if the account exists.
    pub nonce: Option<u64>,
    /// The account balance after execution, if the account exists.
    pub balance: Option<U256>,
    /// The account code hash after execution, if the account exists.
    pub code_hash: Option<B256>,
    /// Storage changes: slot -> present value.
    pub storage: HashMap<B256, U256>,
}
