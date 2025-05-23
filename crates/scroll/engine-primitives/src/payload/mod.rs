//! Engine API Payload types.

mod attributes;
pub use attributes::ScrollPayloadBuilderAttributes;

mod built;
pub use built::ScrollBuiltPayload;

use alloc::{sync::Arc, vec::Vec};
use core::marker::PhantomData;

use alloy_consensus::{proofs, EMPTY_OMMER_ROOT_HASH};
use alloy_eips::eip2718::Decodable2718;
use alloy_primitives::U256;
use alloy_rlp::BufMut;
use alloy_rpc_types_engine::{
    ExecutionData, ExecutionPayload, ExecutionPayloadEnvelopeV2, ExecutionPayloadEnvelopeV3,
    ExecutionPayloadEnvelopeV4, ExecutionPayloadV1, ExecutionPayloadV2, ExecutionPayloadV3,
    PayloadError,
};
use reth_engine_primitives::EngineTypes;
use reth_payload_primitives::{BuiltPayload, PayloadTypes};
use reth_primitives::{Block, BlockBody, Header};
use reth_primitives_traits::{NodePrimitives, SealedBlock};
use reth_scroll_primitives::ScrollBlock;
use scroll_alloy_hardforks::ScrollHardforks;
use scroll_alloy_rpc_types_engine::ScrollPayloadAttributes;

/// The types used in the default Scroll beacon consensus engine.
#[derive(Debug, Default, Clone, serde::Deserialize, serde::Serialize)]
#[non_exhaustive]
pub struct ScrollEngineTypes<T: PayloadTypes = ScrollPayloadTypes> {
    _marker: PhantomData<T>,
}

impl<
        T: PayloadTypes<
            ExecutionData = ExecutionData,
            BuiltPayload: BuiltPayload<Primitives: NodePrimitives<Block = ScrollBlock>>,
        >,
    > PayloadTypes for ScrollEngineTypes<T>
{
    type ExecutionData = T::ExecutionData;
    type BuiltPayload = T::BuiltPayload;
    type PayloadAttributes = T::PayloadAttributes;
    type PayloadBuilderAttributes = T::PayloadBuilderAttributes;

    fn block_to_payload(
        block: SealedBlock<
            <<Self::BuiltPayload as BuiltPayload>::Primitives as NodePrimitives>::Block,
        >,
    ) -> ExecutionData {
        let (payload, sidecar) =
            ExecutionPayload::from_block_unchecked(block.hash(), &block.into_block());
        ExecutionData { payload, sidecar }
    }
}

impl<T> EngineTypes for ScrollEngineTypes<T>
where
    T: PayloadTypes<ExecutionData = ExecutionData>,
    T::BuiltPayload: BuiltPayload<Primitives: NodePrimitives<Block = ScrollBlock>>
        + TryInto<ExecutionPayloadV1>
        + TryInto<ExecutionPayloadEnvelopeV2>
        + TryInto<ExecutionPayloadEnvelopeV3>
        + TryInto<ExecutionPayloadEnvelopeV4>,
{
    type ExecutionPayloadEnvelopeV1 = ExecutionPayloadV1;
    type ExecutionPayloadEnvelopeV2 = ExecutionPayloadEnvelopeV2;
    type ExecutionPayloadEnvelopeV3 = ExecutionPayloadEnvelopeV3;
    type ExecutionPayloadEnvelopeV4 = ExecutionPayloadEnvelopeV4;
}

/// A default payload type for [`ScrollEngineTypes`]
#[derive(Debug, Default, Clone, serde::Deserialize, serde::Serialize)]
#[non_exhaustive]
pub struct ScrollPayloadTypes;

impl PayloadTypes for ScrollPayloadTypes {
    type ExecutionData = ExecutionData;
    type BuiltPayload = ScrollBuiltPayload;
    type PayloadAttributes = ScrollPayloadAttributes;
    type PayloadBuilderAttributes = ScrollPayloadBuilderAttributes;

    fn block_to_payload(
        block: SealedBlock<
            <<Self::BuiltPayload as BuiltPayload>::Primitives as NodePrimitives>::Block,
        >,
    ) -> Self::ExecutionData {
        let (payload, sidecar) =
            ExecutionPayload::from_block_unchecked(block.hash(), &block.into_block());
        ExecutionData { payload, sidecar }
    }
}

/// Tries to create a new unsealed block from the given payload, sidecar and chain specification.
/// Sets the base fee of the block to `None` before the Curie hardfork.
/// Scroll implementation of the [`ExecutionPayload::try_into_block`], which will fail with
/// [`PayloadError::ExtraData`] due to the Scroll blocks containing extra data for the Clique
/// consensus.
pub fn try_into_block<T: Decodable2718, CS: ScrollHardforks>(
    value: ExecutionData,
    chainspec: Arc<CS>,
) -> Result<Block<T>, PayloadError> {
    let mut block = match value.payload {
        ExecutionPayload::V1(payload) => try_payload_v1_to_block(payload, chainspec)?,
        ExecutionPayload::V2(payload) => try_payload_v2_to_block(payload, chainspec)?,
        ExecutionPayload::V3(payload) => try_payload_v3_to_block(payload, chainspec)?,
    };

    block.header.parent_beacon_block_root = value.sidecar.parent_beacon_block_root();
    block.header.requests_hash = value.sidecar.requests_hash();

    Ok(block)
}

/// Tries to convert an [`ExecutionPayloadV1`] to [`Block`].
fn try_payload_v1_to_block<T: Decodable2718, CS: ScrollHardforks>(
    payload: ExecutionPayloadV1,
    chainspec: CS,
) -> Result<Block<T>, PayloadError> {
    // WARNING: It’s allowed for a base fee in EIP1559 to increase unbounded. We assume that
    // it will fit in an u64. This is not always necessarily true, although it is extremely
    // unlikely not to be the case, a u64 maximum would have 2^64 which equates to 18 ETH per
    // gas.
    let basefee = chainspec
        .is_curie_active_at_block(payload.block_number)
        .then_some(payload.base_fee_per_gas)
        .map(|b| b.try_into())
        .transpose()
        .map_err(|_| PayloadError::BaseFee(payload.base_fee_per_gas))?;

    let transactions = payload
        .transactions
        .iter()
        .map(|tx| {
            let mut buf = tx.as_ref();

            let tx = T::decode_2718(&mut buf).map_err(alloy_rlp::Error::from)?;

            if !buf.is_empty() {
                return Err(alloy_rlp::Error::UnexpectedLength);
            }

            Ok(tx)
        })
        .collect::<Result<Vec<_>, _>>()?;

    // Reuse the encoded bytes for root calculation
    let transactions_root =
        proofs::ordered_trie_root_with_encoder(&payload.transactions, |item, buf| {
            buf.put_slice(item)
        });

    let header = Header {
        parent_hash: payload.parent_hash,
        beneficiary: payload.fee_recipient,
        state_root: payload.state_root,
        transactions_root,
        receipts_root: payload.receipts_root,
        withdrawals_root: None,
        logs_bloom: payload.logs_bloom,
        number: payload.block_number,
        gas_limit: payload.gas_limit,
        gas_used: payload.gas_used,
        timestamp: payload.timestamp,
        mix_hash: payload.prev_randao,
        base_fee_per_gas: basefee,
        blob_gas_used: None,
        excess_blob_gas: None,
        parent_beacon_block_root: None,
        requests_hash: None,
        extra_data: payload.extra_data,
        // Defaults
        ommers_hash: EMPTY_OMMER_ROOT_HASH,
        difficulty: U256::ONE,
        nonce: Default::default(),
    };

    Ok(Block { header, body: BlockBody { transactions, ..Default::default() } })
}

/// Tries to convert an [`ExecutionPayloadV2`] to [`Block`].
fn try_payload_v2_to_block<T: Decodable2718, CS: ScrollHardforks>(
    payload: ExecutionPayloadV2,
    chainspec: CS,
) -> Result<Block<T>, PayloadError> {
    // this performs the same conversion as the underlying V1 payload, but calculates the
    // withdrawals root and adds withdrawals
    let mut base_sealed_block = try_payload_v1_to_block(payload.payload_inner, chainspec)?;
    let withdrawals_root = proofs::calculate_withdrawals_root(&payload.withdrawals);
    base_sealed_block.body.withdrawals = Some(payload.withdrawals.into());
    base_sealed_block.header.withdrawals_root = Some(withdrawals_root);
    Ok(base_sealed_block)
}

/// Tries to convert an [`ExecutionPayloadV3`] to [`Block`].
fn try_payload_v3_to_block<T: Decodable2718, CS: ScrollHardforks>(
    payload: ExecutionPayloadV3,
    chainspec: CS,
) -> Result<Block<T>, PayloadError> {
    // this performs the same conversion as the underlying V2 payload, but inserts the blob gas
    // used and excess blob gas
    let mut base_block = try_payload_v2_to_block(payload.payload_inner, chainspec)?;

    base_block.header.blob_gas_used = Some(payload.blob_gas_used);
    base_block.header.excess_blob_gas = Some(payload.excess_blob_gas);

    Ok(base_block)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{Address, Bloom, B256, U256};
    use alloy_rpc_types_engine::ExecutionPayloadV1;
    use arbitrary::{Arbitrary, Unstructured};
    use rand::Rng;
    use reth_scroll_chainspec::SCROLL_MAINNET;
    use reth_scroll_primitives::ScrollTransactionSigned;

    #[test]
    fn test_can_convert_execution_v1_payload_into_block() -> eyre::Result<()> {
        let mut bytes = [0u8; 1024];
        rand::rng().fill(bytes.as_mut_slice());
        let mut u = Unstructured::new(&bytes);

        let mut extra_data = [0u8; 64];
        rand::rng().fill(extra_data.as_mut_slice());

        let execution_payload = ExecutionPayload::V1(ExecutionPayloadV1 {
            parent_hash: B256::random(),
            fee_recipient: Address::random(),
            state_root: B256::random(),
            receipts_root: B256::random(),
            logs_bloom: Bloom::random(),
            prev_randao: B256::random(),
            block_number: u64::arbitrary(&mut u)?,
            gas_limit: u64::arbitrary(&mut u)?,
            gas_used: u64::arbitrary(&mut u)?,
            timestamp: u64::arbitrary(&mut u)?,
            extra_data: extra_data.into(),
            base_fee_per_gas: U256::from(u64::arbitrary(&mut u)?),
            block_hash: B256::random(),
            transactions: vec![],
        });
        let execution_data = ExecutionData::new(execution_payload, Default::default());

        let _: Block<ScrollTransactionSigned> =
            try_into_block(execution_data, SCROLL_MAINNET.clone())?;

        Ok(())
    }

    #[test]
    fn test_can_convert_execution_v2_payload_into_block() -> eyre::Result<()> {
        let mut bytes = [0u8; 1024];
        rand::rng().fill(bytes.as_mut_slice());
        let mut u = Unstructured::new(&bytes);

        let mut extra_data = [0u8; 64];
        rand::rng().fill(extra_data.as_mut_slice());

        let execution_payload = ExecutionPayload::V2(ExecutionPayloadV2 {
            payload_inner: ExecutionPayloadV1 {
                parent_hash: B256::random(),
                fee_recipient: Address::random(),
                state_root: B256::random(),
                receipts_root: B256::random(),
                logs_bloom: Bloom::random(),
                prev_randao: B256::random(),
                block_number: u64::arbitrary(&mut u)?,
                gas_limit: u64::arbitrary(&mut u)?,
                gas_used: u64::arbitrary(&mut u)?,
                timestamp: u64::arbitrary(&mut u)?,
                extra_data: extra_data.into(),
                base_fee_per_gas: U256::from(u64::arbitrary(&mut u)?),
                block_hash: B256::random(),
                transactions: vec![],
            },
            withdrawals: vec![],
        });
        let execution_data = ExecutionData::new(execution_payload, Default::default());

        let _: Block<ScrollTransactionSigned> =
            try_into_block(execution_data, SCROLL_MAINNET.clone())?;

        Ok(())
    }

    #[test]
    fn test_can_convert_execution_v3_payload_into_block() -> eyre::Result<()> {
        let mut bytes = [0u8; 1024];
        rand::rng().fill(bytes.as_mut_slice());
        let mut u = Unstructured::new(&bytes);

        let mut extra_data = [0u8; 64];
        rand::rng().fill(extra_data.as_mut_slice());

        let execution_payload = ExecutionPayload::V3(ExecutionPayloadV3 {
            payload_inner: ExecutionPayloadV2 {
                payload_inner: ExecutionPayloadV1 {
                    parent_hash: B256::random(),
                    fee_recipient: Address::random(),
                    state_root: B256::random(),
                    receipts_root: B256::random(),
                    logs_bloom: Bloom::random(),
                    prev_randao: B256::random(),
                    block_number: u64::arbitrary(&mut u)?,
                    gas_limit: u64::arbitrary(&mut u)?,
                    gas_used: u64::arbitrary(&mut u)?,
                    timestamp: u64::arbitrary(&mut u)?,
                    extra_data: extra_data.into(),
                    base_fee_per_gas: U256::from(u64::arbitrary(&mut u)?),
                    block_hash: B256::random(),
                    transactions: vec![],
                },
                withdrawals: vec![],
            },
            blob_gas_used: 0,
            excess_blob_gas: 0,
        });
        let execution_data = ExecutionData::new(execution_payload, Default::default());

        let _: Block<ScrollTransactionSigned> =
            try_into_block(execution_data, SCROLL_MAINNET.clone())?;

        Ok(())
    }
}
