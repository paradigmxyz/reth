//! `eth_` Extension traits.

use alloy_eips::BlockId;
use alloy_json_rpc::RpcObject;
use alloy_primitives::{Bytes, B256, U256};
use alloy_rpc_types_eth::erc4337::TransactionConditional;
use alloy_rpc_types_eth::TransactionRequest;
use alloy_eips::BlockNumberOrTag;
use jsonrpsee::{core::RpcResult, proc_macros::rpc};

/// Extension trait for `eth_` namespace for L2s.
#[cfg_attr(not(feature = "client"), rpc(server, namespace = "eth"))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "eth"))]
pub trait L2EthApiExt {
    /// Sends signed transaction with the given condition.
    #[method(name = "sendRawTransactionConditional")]
    async fn send_raw_transaction_conditional(
        &self,
        bytes: Bytes,
        condition: TransactionConditional,
    ) -> RpcResult<B256>;

}

/// Preconfirmation transaction event
#[derive(Debug, Clone, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreconfTxEvent {
    /// Transaction hash
    pub tx_hash: B256,
    /// Preconfirmation status
    pub status: PreconfStatus,
    /// Optional failure message
    pub reason: String,
    /// Predicted L2 block number
    #[serde(with = "alloy_serde::quantity")]
    pub block_height: u64,
    /// Preconfirmation transaction receipt
    pub receipt: PreconfTxReceipt,
}

/// Preconfirmation status
#[derive(Debug, Clone, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum PreconfStatus {
    /// Preconfirmation succeeded
    #[serde(rename = "success")]
    Success,
    /// Preconfirmation failed
    #[serde(rename = "failed")]
    Failed,
    /// Preconfirmation timed out
    #[serde(rename = "timeout")]
    Timeout,
    /// Preconfirmation is waiting
    #[serde(rename = "waiting")]
    Waiting,
}

/// Preconfirmation transaction receipt
#[derive(Debug, Clone, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PreconfTxReceipt {
    /// Event logs
    #[serde(default)]
    pub logs: Vec<PreconfLog>,
}

/// Preconfirmation log entry
#[derive(Debug, Clone, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreconfLog {
    /// Log address
    pub address: alloy_primitives::Address,
    /// Log topics
    pub topics: Vec<B256>,
    /// Log data
    pub data: Bytes,
}

/// Extension trait for `eth_` namespace for Mantle networks.
///
/// This trait provides Mantle-specific RPC methods that extend the standard Ethereum RPC API.
#[cfg_attr(not(feature = "client"), rpc(server, namespace = "eth"))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "eth"))]
pub trait MantleEthApiExt<B: RpcObject> {
    /// Returns a list of blocks in the specified range.
    ///
    /// # Deprecation
    ///
    /// This method is deprecated and will be removed in the next network upgrade.
    ///
    /// # Parameters
    /// - `start_number`: The block number to start from (inclusive).
    /// - `end_number`: The block number to end at (inclusive).
    /// - `full_transactions`: If true, returns full transaction objects, otherwise returns hashes only.
    /// 
    /// # Returns
    /// A list of blocks. Each block is a JSON object containing all fields of a block.
    /// 
    /// # Errors
    /// Returns an error if:
    /// - Start block number is greater than end block number.
    /// - The requested range exceeds 1000 blocks.
    /// - The end block doesn't exist.
    #[deprecated(note = "This method will be removed in the next network upgrade.")]
    #[method(name = "getBlockRange")]
    async fn get_block_range(
        &self,
        start_number: BlockNumberOrTag,
        end_number: BlockNumberOrTag,
        full_transactions: bool,
    ) -> RpcResult<Vec<B>>;

    /// Sends a raw transaction with preconfirmation support.
    /// 
    /// This method submits a signed transaction to the transaction pool and returns
    /// a preconfirmation event indicating the transaction's predicted L2 block.
    /// 
    /// # Parameters
    /// - `bytes`: The raw transaction bytes.
    /// 
    /// # Returns
    /// A preconfirmation event containing:
    /// - Transaction hash
    /// - Status (success/failed/timeout/waiting)
    /// - Reason (optional error message)
    /// - Predicted L2 block number
    /// - Receipt (logs)
    #[method(name = "sendRawTransactionWithPreconf")]
    async fn send_raw_transaction_with_preconf(
        &self,
        bytes: Bytes,
    ) -> RpcResult<PreconfTxEvent>;

    /// Estimates the total transaction cost (L2 execution + L1 data + operator fee) in wei.
    ///
    /// Aligned with geth's `eth_estimateTotalFee`. Only supported for Arsia+ blocks.
    #[method(name = "estimateTotalFee")]
    async fn estimate_total_fee(
        &self,
        request: TransactionRequest,
        block_number: Option<BlockId>,
    ) -> RpcResult<U256>;
}
