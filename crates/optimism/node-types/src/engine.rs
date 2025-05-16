//! ZST that aggregates Optimism engine types.

use alloy_rpc_types_engine::{ExecutionPayloadEnvelopeV2, ExecutionPayloadV1};
use core::marker::PhantomData;
use op_alloy_rpc_types_engine::{
    OpExecutionData, OpExecutionPayloadEnvelopeV3, OpExecutionPayloadEnvelopeV4,
};
use reth_node_api::{BuiltPayload, EngineTypes, NodePrimitives, PayloadTypes};
use reth_optimism_payload_builder::OpPayloadTypes;
use reth_optimism_primitives::OpBlock;
use reth_primitives_traits::SealedBlock;

/// The types used in the optimism beacon consensus engine.
#[derive(Debug, Default, Clone, serde::Deserialize, serde::Serialize)]
#[non_exhaustive]
pub struct OpEngineTypes<T: PayloadTypes = OpPayloadTypes> {
    _marker: PhantomData<T>,
}

impl<
        T: PayloadTypes<
            ExecutionData = OpExecutionData,
            BuiltPayload: BuiltPayload<Primitives: NodePrimitives<Block = OpBlock>>,
        >,
    > PayloadTypes for OpEngineTypes<T>
{
    type ExecutionData = T::ExecutionData;
    type BuiltPayload = T::BuiltPayload;
    type PayloadAttributes = T::PayloadAttributes;
    type PayloadBuilderAttributes = T::PayloadBuilderAttributes;

    fn block_to_payload(
        block: SealedBlock<
            <<Self::BuiltPayload as BuiltPayload>::Primitives as NodePrimitives>::Block,
        >,
    ) -> <T as PayloadTypes>::ExecutionData {
        OpExecutionData::from_block_unchecked(block.hash(), &block.into_block())
    }
}

impl<T: PayloadTypes<ExecutionData = OpExecutionData>> EngineTypes for OpEngineTypes<T>
where
    T::BuiltPayload: BuiltPayload<Primitives: NodePrimitives<Block = OpBlock>>
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
