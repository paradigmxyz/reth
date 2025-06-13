use crate::primitives::WormholeNodePrimitives;
use alloy_rpc_types_engine::{ExecutionPayloadEnvelopeV2, ExecutionPayloadV1};
use op_alloy_rpc_types_engine::{
    OpExecutionData, OpExecutionPayloadEnvelopeV3, OpExecutionPayloadEnvelopeV4,
    OpPayloadAttributes,
};
use reth_node_api::{BuiltPayload, EngineTypes, NodePrimitives, PayloadTypes};
use reth_optimism_payload_builder::{OpBuiltPayload, OpPayloadBuilderAttributes};
use reth_primitives_traits::{Block, SealedBlock};
use std::marker::PhantomData;

/// The types used in the wormhole beacon consensus engine.
#[derive(Debug, Default, Clone, serde::Deserialize, serde::Serialize)]
#[non_exhaustive]
pub struct WormholeEngineTypes<T: PayloadTypes = WormholePayloadTypes> {
    _marker: PhantomData<T>,
}

/// Wormhole payload types that use WormholeNodePrimitives
#[derive(Debug, Default, Clone, serde::Deserialize, serde::Serialize)]
pub struct WormholePayloadTypes;

impl PayloadTypes for WormholePayloadTypes {
    type ExecutionData = OpExecutionData;
    type BuiltPayload = OpBuiltPayload<WormholeNodePrimitives>;
    type PayloadAttributes = OpPayloadAttributes;
    type PayloadBuilderAttributes =
        OpPayloadBuilderAttributes<crate::primitives::WormholeTransactionSigned>;

    fn block_to_payload(
        block: SealedBlock<
            <<Self::BuiltPayload as BuiltPayload>::Primitives as NodePrimitives>::Block,
        >,
    ) -> <Self as PayloadTypes>::ExecutionData {
        OpExecutionData::from_block_unchecked(
            block.hash(),
            &block.into_block().into_ethereum_block(),
        )
    }
}

impl<T: PayloadTypes<ExecutionData = OpExecutionData>> PayloadTypes for WormholeEngineTypes<T> {
    type ExecutionData = T::ExecutionData;
    type BuiltPayload = T::BuiltPayload;
    type PayloadAttributes = T::PayloadAttributes;
    type PayloadBuilderAttributes = T::PayloadBuilderAttributes;

    fn block_to_payload(
        block: SealedBlock<
            <<Self::BuiltPayload as BuiltPayload>::Primitives as NodePrimitives>::Block,
        >,
    ) -> <T as PayloadTypes>::ExecutionData {
        T::block_to_payload(block)
    }
}

impl<T: PayloadTypes<ExecutionData = OpExecutionData>> EngineTypes for WormholeEngineTypes<T>
where
    T::BuiltPayload: BuiltPayload<Primitives = WormholeNodePrimitives>
        + TryInto<ExecutionPayloadV1>
        + TryInto<ExecutionPayloadEnvelopeV2>
        + TryInto<OpExecutionPayloadEnvelopeV3>
        + TryInto<OpExecutionPayloadEnvelopeV4>,
{
    type ExecutionPayloadEnvelopeV1 = ExecutionPayloadV1;
    type ExecutionPayloadEnvelopeV2 = ExecutionPayloadEnvelopeV2;
    type ExecutionPayloadEnvelopeV3 = OpExecutionPayloadEnvelopeV3;
    type ExecutionPayloadEnvelopeV4 = OpExecutionPayloadEnvelopeV4;
    type ExecutionPayloadEnvelopeV5 = OpExecutionPayloadEnvelopeV4;
}
