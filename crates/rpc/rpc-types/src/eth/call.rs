use reth_primitives::{AccessList, Address, BlockId, Bytes, U256, U64, U8};
use serde::{Deserialize, Serialize};

use crate::BlockOverrides;

/// Bundle of transactions
#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Bundle {
    /// Transactions
    pub transactions: Vec<CallRequest>,
    /// Block overides
    pub block_override: Option<BlockOverrides>,
}

/// State context for callMany
#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct StateContext {
    /// Block Number
    pub block_number: Option<BlockId>,
    /// Inclusive number of tx to replay in block. -1 means replay all
    pub transaction_index: Option<isize>,
}

/// Call request
#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CallRequest {
    /// From
    pub from: Option<Address>,
    /// To
    pub to: Option<Address>,
    /// Gas Price
    pub gas_price: Option<U256>,
    /// EIP-1559 Max base fee the caller is willing to pay
    pub max_fee_per_gas: Option<U256>,
    /// EIP-1559 Priority fee the caller is paying to the block author
    pub max_priority_fee_per_gas: Option<U256>,
    /// Gas
    pub gas: Option<U256>,
    /// Value
    pub value: Option<U256>,
    /// Transaction data
    ///
    /// This accepts both `input` and `data`
    #[serde(alias = "input")]
    pub data: Option<Bytes>,
    /// Nonce
    pub nonce: Option<U256>,
    /// chain id
    pub chain_id: Option<U64>,
    /// AccessList
    pub access_list: Option<AccessList>,
    /// EIP-2718 type
    #[serde(rename = "type")]
    pub transaction_type: Option<U8>,
}

impl CallRequest {
    /// Returns the configured fee cap, if any.
    ///
    /// The returns `gas_price` (legacy) if set or `max_fee_per_gas` (EIP1559)
    pub fn fee_cap(&self) -> Option<U256> {
        self.gas_price.or(self.max_fee_per_gas)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_call_request() {
        let s = r#"{"accessList":[],"data":"0x0902f1ac","to":"0xa478c2975ab1ea89e8196811f51a7b7ade33eb11","type":"0x02"}"#;
        let _req = serde_json::from_str::<CallRequest>(s).unwrap();
    }
}
