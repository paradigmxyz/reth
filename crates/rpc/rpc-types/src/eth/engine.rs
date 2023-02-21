//! Engine API types: <https://github.com/ethereum/execution-apis/blob/main/src/engine/authentication.md> and <https://eips.ethereum.org/EIPS/eip-3675> following the execution specs <https://github.com/ethereum/execution-apis/tree/6709c2a795b707202e93c4f2867fa0bf2640a84f/src/engine>

#![allow(missing_docs)]

use reth_primitives::{
    Address, Block, Bloom, Bytes, SealedBlock, Withdrawal, H256, H64, U256, U64,
};
use reth_rlp::Encodable;
use serde::{Deserialize, Serialize};

/// The list of supported Engine capabilities
pub const CAPABILITIES: [&str; 9] = [
    "engine_forkchoiceUpdatedV1",
    "engine_forkchoiceUpdatedV2",
    "engine_exchangeTransitionConfigurationV1",
    "engine_getPayloadV1",
    "engine_getPayloadV2",
    "engine_newPayloadV1",
    "engine_newPayloadV2",
    "engine_getPayloadBodiesByHashV1",
    "engine_getPayloadBodiesByRangeV1",
];

/// This structure maps on the ExecutionPayload structure of the beacon chain spec.
///
/// See also: <https://github.com/ethereum/execution-apis/blob/6709c2a795b707202e93c4f2867fa0bf2640a84f/src/engine/paris.md#executionpayloadv1>
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionPayload {
    pub parent_hash: H256,
    pub fee_recipient: Address,
    pub state_root: H256,
    pub receipts_root: H256,
    pub logs_bloom: Bloom,
    pub prev_randao: H256,
    pub block_number: U64,
    pub gas_limit: U64,
    pub gas_used: U64,
    pub timestamp: U64,
    pub extra_data: Bytes,
    pub base_fee_per_gas: U256,
    pub block_hash: H256,
    pub transactions: Vec<Bytes>,
    /// Array of [`Withdrawal`] enabled with V2
    /// See <https://github.com/ethereum/execution-apis/blob/6709c2a795b707202e93c4f2867fa0bf2640a84f/src/engine/shanghai.md#executionpayloadv2>
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub withdrawals: Option<Vec<Withdrawal>>,
}

impl From<SealedBlock> for ExecutionPayload {
    fn from(value: SealedBlock) -> Self {
        let transactions = value
            .body
            .iter()
            .map(|tx| {
                let mut encoded = Vec::new();
                tx.encode(&mut encoded);
                encoded.into()
            })
            .collect();
        ExecutionPayload {
            parent_hash: value.parent_hash,
            fee_recipient: value.beneficiary,
            state_root: value.state_root,
            receipts_root: value.receipts_root,
            logs_bloom: value.logs_bloom,
            prev_randao: value.mix_hash,
            block_number: value.number.into(),
            gas_limit: value.gas_limit.into(),
            gas_used: value.gas_used.into(),
            timestamp: value.timestamp.into(),
            extra_data: value.extra_data.clone(),
            base_fee_per_gas: U256::from(value.base_fee_per_gas.unwrap_or_default()),
            block_hash: value.hash(),
            transactions,
            withdrawals: value.withdrawals,
        }
    }
}

/// This structure contains a body of an execution payload.
///
/// See also: <https://github.com/ethereum/execution-apis/blob/6452a6b194d7db269bf1dbd087a267251d3cc7f8/src/engine/shanghai.md#executionpayloadbodyv1>
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionPayloadBody {
    pub transactions: Vec<Bytes>,
    pub withdrawals: Vec<Withdrawal>,
}

impl From<Block> for ExecutionPayloadBody {
    fn from(value: Block) -> Self {
        let transactions = value.body.into_iter().map(|tx| {
            let mut out = Vec::new();
            tx.encode(&mut out);
            out.into()
        });
        ExecutionPayloadBody {
            transactions: transactions.collect(),
            withdrawals: value.withdrawals.unwrap_or_default(),
        }
    }
}

/// The execution payload body response that allows for `null` values.
pub type ExecutionPayloadBodies = Vec<Option<ExecutionPayloadBody>>;

/// This structure encapsulates the fork choice state
#[derive(Default, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForkchoiceState {
    pub head_block_hash: H256,
    pub safe_block_hash: H256,
    pub finalized_block_hash: H256,
}

/// This structure contains the attributes required to initiate a payload build process in the
/// context of an `engine_forkchoiceUpdated` call.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PayloadAttributes {
    pub timestamp: U64,
    pub prev_randao: H256,
    pub suggested_fee_recipient: Address,
    /// Array of [`Withdrawal`] enabled with V2
    /// See <https://github.com/ethereum/execution-apis/blob/6452a6b194d7db269bf1dbd087a267251d3cc7f8/src/engine/shanghai.md#payloadattributesv2>
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub withdrawals: Option<Vec<Withdrawal>>,
}

/// This structure contains the result of processing a payload
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PayloadStatus {
    #[serde(flatten)]
    pub status: PayloadStatusEnum,
    /// Hash of the most recent valid block in the branch defined by payload and its ancestors
    pub latest_valid_hash: Option<H256>,
}

impl PayloadStatus {
    pub fn new(status: PayloadStatusEnum, latest_valid_hash: H256) -> Self {
        Self { status, latest_valid_hash: Some(latest_valid_hash) }
    }

    pub fn from_status(status: PayloadStatusEnum) -> Self {
        Self { status, latest_valid_hash: None }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PayloadStatusEnum {
    Valid,
    Invalid {
        #[serde(rename = "validationError")]
        validation_error: String,
    },
    Syncing,
    Accepted,
    InvalidBlockHash {
        #[serde(rename = "validationError")]
        validation_error: String,
    },
}

/// This structure contains configurable settings of the transition process.
#[derive(Default, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransitionConfiguration {
    /// Maps on the TERMINAL_TOTAL_DIFFICULTY parameter of EIP-3675
    pub terminal_total_difficulty: U256,
    /// Maps on TERMINAL_BLOCK_HASH parameter of EIP-3675
    pub terminal_block_hash: H256,
    /// Maps on TERMINAL_BLOCK_NUMBER parameter of EIP-3675
    pub terminal_block_number: U64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForkchoiceUpdated {
    pub payload_status: PayloadStatus,
    pub payload_id: Option<H64>,
}

impl ForkchoiceUpdated {
    pub fn new(payload_status: PayloadStatus) -> Self {
        Self { payload_status, payload_id: None }
    }

    pub fn from_status(status: PayloadStatusEnum) -> Self {
        Self { payload_status: PayloadStatus::from_status(status), payload_id: None }
    }

    pub fn with_latest_valid_hash(mut self, hash: H256) -> Self {
        self.payload_status.latest_valid_hash = Some(hash);
        self
    }

    pub fn with_payload_id(mut self, id: H64) -> Self {
        self.payload_id = Some(id);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_interfaces::test_utils::generators::random_block_range;
    use reth_primitives::{TransactionSigned, H256};
    use reth_rlp::Decodable;

    #[test]
    fn payload_body_roundtrip() {
        for block in random_block_range(0..100, H256::default(), 0..2) {
            let unsealed = block.clone().unseal();
            let payload_body: ExecutionPayloadBody = unsealed.into();

            assert_eq!(
                Ok(block.body),
                payload_body
                    .transactions
                    .iter()
                    .map(|x| TransactionSigned::decode(&mut &x[..]))
                    .collect::<Result<Vec<_>, _>>(),
            );

            assert_eq!(block.withdrawals.unwrap_or_default(), payload_body.withdrawals);
        }
    }
}
