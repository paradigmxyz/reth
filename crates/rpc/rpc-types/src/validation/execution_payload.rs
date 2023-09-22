pub use crate::{
    eth::engine::{ExecutionPayloadV1, ExecutionPayloadV2},
    ExecutionPayload, Withdrawal,
};
use derivative::Derivative;
use reth_primitives::{Address, Bloom, Bytes, H256, U256, U64};
use serde::{ser::SerializeMap, Deserialize, Serialize, Serializer};
use serde_this_or_that::as_u64;

/// Structure to deserialize execution payloads sent according to the builder api spec
#[derive(Derivative)]
#[derivative(Debug)]
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionPayloadValidation {
    pub parent_hash: H256,
    pub fee_recipient: Address,
    pub state_root: H256,
    pub receipts_root: H256,
    pub logs_bloom: Bloom,
    pub prev_randao: H256,
    #[serde(deserialize_with = "as_u64")]
    pub block_number: u64,
    #[serde(deserialize_with = "as_u64")]
    pub gas_limit: u64,
    #[serde(deserialize_with = "as_u64")]
    pub gas_used: u64,
    #[serde(deserialize_with = "as_u64")]
    pub timestamp: u64,
    pub extra_data: Bytes,
    pub base_fee_per_gas: U256,
    pub block_hash: H256,
    #[derivative(Debug = "ignore")]
    pub transactions: Vec<Bytes>,
    pub withdrawals: Vec<WithdrawalValidation>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WithdrawalValidation {
    /// Monotonically increasing identifier issued by consensus layer.
    #[serde(deserialize_with = "as_u64")]
    pub index: u64,
    /// Index of validator associated with withdrawal.
    #[serde(deserialize_with = "as_u64")]
    pub validator_index: u64,
    /// Target address for withdrawn ether.
    pub address: Address,
    /// Value of the withdrawal in gwei.
    #[serde(deserialize_with = "as_u64")]
    pub amount: u64,
}

impl Into<ExecutionPayload> for ExecutionPayloadValidation {
    fn into(self) -> ExecutionPayload {
        ExecutionPayload::V2(ExecutionPayloadV2 {
            payload_inner: ExecutionPayloadV1 {
                parent_hash: self.parent_hash,
                fee_recipient: self.fee_recipient,
                state_root: self.state_root,
                receipts_root: self.receipts_root,
                logs_bloom: self.logs_bloom,
                prev_randao: self.prev_randao,
                block_number: self.block_number.into(),
                gas_limit: self.gas_limit.into(),
                gas_used: self.gas_used.into(),
                timestamp: self.timestamp.into(),
                extra_data: self.extra_data,
                base_fee_per_gas: self.base_fee_per_gas,
                block_hash: self.block_hash,
                transactions: self.transactions,
            },
            withdrawals: self.withdrawals.into_iter().map(|w| w.into()).collect(),
        })
    }
}

impl Into<Withdrawal> for WithdrawalValidation {
    fn into(self) -> Withdrawal {
        Withdrawal {
            index: self.index,
            validator_index: self.validator_index,
            address: self.address,
            amount: self.amount,
        }
    }
}
