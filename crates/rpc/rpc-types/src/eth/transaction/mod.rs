mod receipt;
mod request;
mod typed;

pub use receipt::TransactionReceipt;
pub use request::TransactionRequest;
pub use typed::*;

use reth_primitives::{
    rpc::transaction::eip2930::AccessListItem, Address, Bytes, H256, H512, U128, U256, U64,
};
use serde::{Deserialize, Serialize};

/// Transaction object
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
    /// Max BaseFeePerGas the user is willing to pay.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_fee_per_gas: Option<U128>,
    /// The miner's tip.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_priority_fee_per_gas: Option<U128>,
    /// Gas
    pub gas: U256,
    /// Data
    pub input: Bytes,
    /// Creates contract
    pub creates: Option<Address>,
    /// Raw transaction data
    pub raw: Bytes,
    /// Public key of the signer.
    pub public_key: Option<H512>,
    /// The network id of the transaction, if any.
    pub chain_id: Option<U64>,
    /// The standardised V field of the signature (0 or 1).
    pub standard_v: U256,
    /// The standardised V field of the signature.
    pub v: U256,
    /// The R field of the signature.
    pub r: U256,
    /// The S field of the signature.
    pub s: U256,
    /// Pre-pay to warm storage access.
    #[cfg_attr(feature = "std", serde(skip_serializing_if = "Option::is_none"))]
    pub access_list: Option<Vec<AccessListItem>>,
    /// EIP-2718 type
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub transaction_type: Option<U256>,
}
