mod receipt;
mod request;
mod signature;
mod typed;

pub use receipt::TransactionReceipt;
pub use request::TransactionRequest;
pub use signature::Signature;
pub use typed::*;

use reth_primitives::{
    rpc::transaction::eip2930::AccessListItem, Address, Bytes, H256, U128, U256, U64,
};
use serde::{Deserialize, Serialize};

/// Transaction object used in RPC
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Transaction {
    /// Hash
    pub hash: H256,
    /// Nonce
    pub nonce: U256,
    /// Block hash
    pub block_hash: Option<H256>,
    /// Block number
    pub block_number: Option<U256>,
    /// Transaction Index
    pub transaction_index: Option<U256>,
    /// Sender
    pub from: Address,
    /// Recipient
    pub to: Option<Address>,
    /// Transferred value
    pub value: U256,
    /// Gas Price
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gas_price: Option<U128>,
    /// Gas amount
    pub gas: U256,
    /// Max BaseFeePerGas the user is willing to pay.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_fee_per_gas: Option<U128>,
    /// The miner's tip.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_priority_fee_per_gas: Option<U128>,
    /// Data
    pub input: Bytes,
    /// All _flattened_ fields of the transaction signature.
    ///
    /// Note: this is an option so special transaction types without a signature (e.g. <https://github.com/ethereum-optimism/optimism/blob/0bf643c4147b43cd6f25a759d331ef3a2a61a2a3/specs/deposits.md#the-deposited-transaction-type>) can be supported.
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub signature: Option<Signature>,
    /// The chain id of the transaction, if any.
    pub chain_id: Option<U64>,
    /// EIP2930
    ///
    /// Pre-pay to warm storage access.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_list: Option<Vec<AccessListItem>>,
    /// EIP2718
    ///
    /// Transaction type, Some(2) for EIP-1559 transaction,
    /// Some(1) for AccessList transaction, None for Legacy
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub transaction_type: Option<U64>,
}
