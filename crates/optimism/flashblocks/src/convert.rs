//! Conversions from flashblock types to Engine API payload types.
//!
//! This module provides the canonical implementations for converting flashblock data structures
//! into the various versions of execution payloads required by the Engine API.
//!
//! TODO: ideally we move this to op-alloy-rpc-types-engine where `from_block_unchecked` is defined
//! and define a similar called `from_flashblock_sequence_unchecked`.
//! The goal is to have the conversion logic in one place to avoid missing steps.

use crate::{
    ExecutionPayloadBaseV1, ExecutionPayloadFlashblockDeltaV1, FlashBlockCompleteSequence,
};
use alloy_rpc_types_engine::{ExecutionPayloadV1, ExecutionPayloadV2, ExecutionPayloadV3};
use op_alloy_rpc_types_engine::{OpExecutionData, OpExecutionPayloadV4};

/// Converts flashblock base and delta into `ExecutionPayloadV1`.
///
/// This is the foundational conversion that constructs the base execution payload
/// from the static base fields and the accumulated delta state.
fn to_execution_payload_v1(
    base: &ExecutionPayloadBaseV1,
    delta: &ExecutionPayloadFlashblockDeltaV1,
) -> ExecutionPayloadV1 {
    ExecutionPayloadV1 {
        parent_hash: base.parent_hash,
        fee_recipient: base.fee_recipient,
        state_root: delta.state_root,
        receipts_root: delta.receipts_root,
        logs_bloom: delta.logs_bloom,
        prev_randao: base.prev_randao,
        block_number: base.block_number,
        gas_limit: base.gas_limit,
        gas_used: delta.gas_used,
        timestamp: base.timestamp,
        extra_data: base.extra_data.clone(),
        base_fee_per_gas: base.base_fee_per_gas,
        block_hash: delta.block_hash,
        transactions: delta.transactions.clone(),
    }
}

/// Converts flashblock base and delta into `ExecutionPayloadV2`.
///
/// Wraps V1 payload with empty withdrawals array, as Optimism L2 blocks
/// don't use Ethereum L1 withdrawals.
fn to_execution_payload_v2(
    base: &ExecutionPayloadBaseV1,
    delta: &ExecutionPayloadFlashblockDeltaV1,
) -> ExecutionPayloadV2 {
    ExecutionPayloadV2 {
        payload_inner: to_execution_payload_v1(base, delta),
        withdrawals: delta.withdrawals.clone(),
    }
}

/// Converts flashblock base and delta into `ExecutionPayloadV3`.
///
/// Wraps V2 payload with blob gas fields set to zero, as flashblocks
/// currently don't track blob gas usage.
fn to_execution_payload_v3(
    base: &ExecutionPayloadBaseV1,
    delta: &ExecutionPayloadFlashblockDeltaV1,
) -> ExecutionPayloadV3 {
    ExecutionPayloadV3 {
        payload_inner: to_execution_payload_v2(base, delta),
        // TODO: update when ExecutionPayloadFlashblockDeltaV1 contains `blob_gas_used` after Jovian
        blob_gas_used: 0,
        excess_blob_gas: 0,
    }
}

/// Converts flashblock base and delta into `OpExecutionPayloadV4`.
///
/// Wraps V3 payload with the withdrawals root from the flashblock delta.
fn to_execution_payload_v4(
    base: &ExecutionPayloadBaseV1,
    delta: &ExecutionPayloadFlashblockDeltaV1,
) -> OpExecutionPayloadV4 {
    OpExecutionPayloadV4 {
        payload_inner: to_execution_payload_v3(base, delta),
        withdrawals_root: delta.withdrawals_root,
    }
}

/// Converts a complete flashblock sequence into `OpExecutionData`.
///
/// This is the primary conversion used when submitting a completed flashblock sequence
/// to the Engine API via `engine_newPayloadV4`. It combines the base payload fields
/// from the first flashblock with the accumulated state from the last flashblock.
impl From<&FlashBlockCompleteSequence> for OpExecutionData {
    fn from(sequence: &FlashBlockCompleteSequence) -> Self {
        let base = sequence.payload_base();
        let last = sequence.last();

        // Build the V4 payload using the conversion functions
        let payload_v4 = to_execution_payload_v4(base, &last.diff);

        // Use OpExecutionData::v4 helper to construct the complete data with sidecar
        Self::v4(payload_v4, vec![], base.parent_beacon_block_root, Default::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{ruint::aliases::U256, Address, Bloom, Bytes, B256};

    fn sample_base() -> ExecutionPayloadBaseV1 {
        ExecutionPayloadBaseV1 {
            parent_beacon_block_root: B256::random(),
            parent_hash: B256::random(),
            fee_recipient: Address::random(),
            prev_randao: B256::random(),
            block_number: 100,
            gas_limit: 30_000_000,
            timestamp: 1234567890,
            extra_data: Bytes::from(vec![1, 2, 3]),
            base_fee_per_gas: U256::random(),
        }
    }

    fn sample_delta() -> ExecutionPayloadFlashblockDeltaV1 {
        ExecutionPayloadFlashblockDeltaV1 {
            state_root: B256::random(),
            receipts_root: B256::random(),
            logs_bloom: Bloom::default(),
            gas_used: 21_000,
            block_hash: B256::random(),
            transactions: vec![],
            withdrawals: vec![],
            withdrawals_root: B256::random(),
        }
    }

    #[test]
    fn test_v1_conversion() {
        let base = sample_base();
        let delta = sample_delta();

        let payload = to_execution_payload_v1(&base, &delta);

        assert_eq!(payload.parent_hash, base.parent_hash);
        assert_eq!(payload.fee_recipient, base.fee_recipient);
        assert_eq!(payload.state_root, delta.state_root);
        assert_eq!(payload.receipts_root, delta.receipts_root);
        assert_eq!(payload.gas_used, delta.gas_used);
        assert_eq!(payload.block_hash, delta.block_hash);
    }

    #[test]
    fn test_v2_conversion() {
        let base = sample_base();
        let delta = sample_delta();

        let payload = to_execution_payload_v2(&base, &delta);

        assert_eq!(payload.payload_inner.parent_hash, base.parent_hash);
        assert!(payload.withdrawals.is_empty());
    }

    #[test]
    fn test_v3_conversion() {
        let base = sample_base();
        let delta = sample_delta();

        let payload = to_execution_payload_v3(&base, &delta);

        assert_eq!(payload.payload_inner.payload_inner.parent_hash, base.parent_hash);
        assert_eq!(payload.blob_gas_used, 0);
        assert_eq!(payload.excess_blob_gas, 0);
    }

    #[test]
    fn test_v4_conversion() {
        let base = sample_base();
        let delta = sample_delta();

        let payload = to_execution_payload_v4(&base, &delta);

        assert_eq!(payload.payload_inner.payload_inner.payload_inner.parent_hash, base.parent_hash);
        assert_eq!(payload.withdrawals_root, delta.withdrawals_root);
    }
}
