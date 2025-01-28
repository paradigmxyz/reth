//! Engine API Payload types.

mod attributes;
pub use attributes::ScrollPayloadBuilderAttributes;

mod built;
pub use built::ScrollBuiltPayload;

use alloc::sync::Arc;
use core::marker::PhantomData;

use alloy_consensus::Block;
use alloy_eips::eip2718::Decodable2718;
use alloy_rpc_types_engine::{
    ExecutionPayload, ExecutionPayloadEnvelopeV2, ExecutionPayloadEnvelopeV3,
    ExecutionPayloadEnvelopeV4, ExecutionPayloadSidecar, ExecutionPayloadV1, PayloadError,
};
use reth_engine_primitives::EngineTypes;
use reth_payload_primitives::{BuiltPayload, PayloadTypes};
use reth_primitives_traits::{NodePrimitives, SealedBlock};
use reth_rpc_types_compat::engine::payload::block_to_payload;
use reth_scroll_chainspec::ScrollChainSpec;
use reth_scroll_forks::ScrollHardfork;
use reth_scroll_primitives::ScrollBlock;
use scroll_alloy_rpc_types_engine::ScrollPayloadAttributes;

/// The types used in the default Scroll beacon consensus engine.
#[derive(Debug, Default, Clone, serde::Deserialize, serde::Serialize)]
#[non_exhaustive]
pub struct ScrollEngineTypes<T: PayloadTypes = ScrollPayloadTypes> {
    _marker: PhantomData<T>,
}

impl<T: PayloadTypes> PayloadTypes for ScrollEngineTypes<T> {
    type BuiltPayload = T::BuiltPayload;
    type PayloadAttributes = T::PayloadAttributes;
    type PayloadBuilderAttributes = T::PayloadBuilderAttributes;
}

impl<T> EngineTypes for ScrollEngineTypes<T>
where
    T: PayloadTypes,
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

    fn block_to_payload(
        block: SealedBlock<
            <<Self::BuiltPayload as BuiltPayload>::Primitives as NodePrimitives>::Block,
        >,
    ) -> (ExecutionPayload, ExecutionPayloadSidecar) {
        block_to_payload(block)
    }
}

/// A default payload type for [`ScrollEngineTypes`]
#[derive(Debug, Default, Clone, serde::Deserialize, serde::Serialize)]
#[non_exhaustive]
pub struct ScrollPayloadTypes;

impl PayloadTypes for ScrollPayloadTypes {
    type BuiltPayload = ScrollBuiltPayload;
    type PayloadAttributes = ScrollPayloadAttributes;
    type PayloadBuilderAttributes = ScrollPayloadBuilderAttributes;
}

/// Tries to create a new unsealed block from the given payload, sidecar and chain specification.
/// Sets the base fee of the block to `None` before the Curie hardfork.
pub fn try_into_block<T: Decodable2718>(
    payload: ExecutionPayload,
    sidecar: &ExecutionPayloadSidecar,
    chainspec: Arc<ScrollChainSpec>,
) -> Result<Block<T>, PayloadError> {
    let mut block = payload.try_into_block_with_sidecar(sidecar)?;

    let basefee = chainspec
        .is_fork_active_at_block(ScrollHardfork::Curie, block.number)
        .then(|| block.base_fee_per_gas)
        .flatten();

    block.header.base_fee_per_gas = basefee;

    Ok(block)
}
