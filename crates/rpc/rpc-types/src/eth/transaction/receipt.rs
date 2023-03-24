use crate::Log;
use reth_primitives::{Address, Bloom, H256, U128, U256, U64};
use serde::{Deserialize, Serialize};

/// Transaction receipt
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionReceipt {
    /// Transaction Hash.
    pub transaction_hash: Option<H256>,
    /// Index within the block.
    pub transaction_index: Option<U256>,
    /// Hash of the block this transaction was included within.
    pub block_hash: Option<H256>,
    /// Number of the block this transaction was included within.
    pub block_number: Option<U256>,
    /// Address of the sender
    pub from: Address,
    /// Address of the receiver. null when its a contract creation transaction.
    pub to: Option<Address>,
    /// Cumulative gas used within the block after this was executed.
    pub cumulative_gas_used: U256,
    /// Gas used by this transaction alone.
    pub gas_used: Option<U256>,
    /// Contract address created, or None if not a deployment.
    pub contract_address: Option<Address>,
    /// Logs emitted by this transaction.
    pub logs: Vec<Log>,
    /// The post-transaction stateroot (pre Byzantium)
    ///
    /// EIP98 makes this optional field, if it's missing then skip serializing it
    #[serde(skip_serializing_if = "Option::is_none", rename = "root")]
    pub state_root: Option<H256>,
    /// Logs bloom
    pub logs_bloom: Bloom,
    /// Status: either 1 (success) or 0 (failure). Only present after activation of EIP-658
    #[serde(skip_serializing_if = "Option::is_none", rename = "status")]
    pub status_code: Option<U64>,
    /// The price paid post-execution by the transaction (i.e. base fee + priority fee). Both
    /// fields in 1559-style transactions are maximums (max fee + max priority fee), the amount
    /// that's actually paid by users can only be determined post-execution
    pub effective_gas_price: U128,
    /// EIP-2718 Transaction type, Some(1) for AccessList transaction, None for Legacy
    #[serde(rename = "type")]
    pub transaction_type: U256,
}
