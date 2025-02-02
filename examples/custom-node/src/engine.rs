use crate::primitives::CustomNodePrimitives;
use alloy_consensus::{Block, BlockBody};
use alloy_rpc_types_engine::{
    BlobsBundleV1, ExecutionPayload, ExecutionPayloadV1, ExecutionPayloadV2, ExecutionPayloadV3,
};
use op_alloy_rpc_types_engine::{OpExecutionPayloadEnvelopeV3, OpExecutionPayloadEnvelopeV4};
use reth_chain_state::ExecutedBlockWithTrieUpdates;
use reth_node_api::{BuiltPayload, EngineTypes, NodePrimitives, PayloadTypes};
use reth_optimism_node::{
    OpBuiltPayload, OpEngineTypes, OpPayloadAttributes, OpPayloadBuilderAttributes,
};
use reth_optimism_primitives::{OpBlock, OpTransactionSigned};
use reth_primitives_traits::SealedBlock;
use revm_primitives::U256;

#[derive(Debug, Clone, Copy)]
pub struct CustomEngineTypes;

#[derive(Debug, Clone)]
pub struct CustomBuiltPayload(OpBuiltPayload<CustomNodePrimitives>);

impl BuiltPayload for CustomBuiltPayload {
    type Primitives = CustomNodePrimitives;

    fn block(&self) -> &SealedBlock<<Self::Primitives as NodePrimitives>::Block> {
        self.0.block()
    }

    fn executed_block(&self) -> Option<ExecutedBlockWithTrieUpdates<Self::Primitives>> {
        self.0.executed_block()
    }

    fn fees(&self) -> U256 {
        self.0.fees()
    }

    fn requests(&self) -> Option<alloy_eips::eip7685::Requests> {
        self.0.requests()
    }
}

impl From<CustomBuiltPayload>
    for alloy_consensus::Block<<CustomNodePrimitives as NodePrimitives>::SignedTx>
{
    fn from(value: CustomBuiltPayload) -> Self {
        value.0.into_sealed_block().into_block().map_header(|header| header.inner)
    }
}

impl From<CustomBuiltPayload> for ExecutionPayloadV1 {
    fn from(value: CustomBuiltPayload) -> Self {
        Self::from_block_unchecked(value.block().hash(), &value.into())
    }
}

impl From<CustomBuiltPayload> for ExecutionPayloadV2 {
    fn from(value: CustomBuiltPayload) -> Self {
        Self::from_block_unchecked(value.block().hash(), &value.into())
    }
}

impl From<CustomBuiltPayload> for OpExecutionPayloadEnvelopeV3 {
    fn from(value: CustomBuiltPayload) -> Self {
        Self {
            block_value: value.fees(),
            // From the engine API spec:
            //
            // > Client software **MAY** use any heuristics to decide whether to set
            // `shouldOverrideBuilder` flag or not. If client software does not implement any
            // heuristic this flag **SHOULD** be set to `false`.
            //
            // Spec:
            // <https://github.com/ethereum/execution-apis/blob/fe8e13c288c592ec154ce25c534e26cb7ce0530d/src/engine/cancun.md#specification-2>
            should_override_builder: false,
            // No blobs for OP.
            blobs_bundle: BlobsBundleV1 { blobs: vec![], commitments: vec![], proofs: vec![] },
            parent_beacon_block_root: value.0.block().parent_beacon_block_root.unwrap_or_default(),
            execution_payload: ExecutionPayloadV3::from_block_unchecked(
                value.0.block().hash(),
                &value.into(),
            ),
        }
    }
}

impl From<CustomBuiltPayload> for OpExecutionPayloadEnvelopeV4 {
    fn from(value: CustomBuiltPayload) -> Self {
        Self {
            block_value: value.fees(),
            // From the engine API spec:
            //
            // > Client software **MAY** use any heuristics to decide whether to set
            // `shouldOverrideBuilder` flag or not. If client software does not implement any
            // heuristic this flag **SHOULD** be set to `false`.
            //
            // Spec:
            // <https://github.com/ethereum/execution-apis/blob/fe8e13c288c592ec154ce25c534e26cb7ce0530d/src/engine/cancun.md#specification-2>
            should_override_builder: false,
            // No blobs for OP.
            blobs_bundle: BlobsBundleV1 { blobs: vec![], commitments: vec![], proofs: vec![] },
            parent_beacon_block_root: value.0.block().parent_beacon_block_root.unwrap_or_default(),
            execution_payload: ExecutionPayloadV3::from_block_unchecked(
                value.0.block().hash(),
                &value.into(),
            ),
            execution_requests: vec![],
        }
    }
}

impl PayloadTypes for CustomEngineTypes {
    type BuiltPayload = CustomBuiltPayload;
    type PayloadAttributes = OpPayloadAttributes;
    type PayloadBuilderAttributes = OpPayloadBuilderAttributes<OpTransactionSigned>;
}

impl EngineTypes for CustomEngineTypes {
    type ExecutionPayloadEnvelopeV1 = ExecutionPayloadV1;
    type ExecutionPayloadEnvelopeV2 = ExecutionPayloadV2;
    type ExecutionPayloadEnvelopeV3 = OpExecutionPayloadEnvelopeV3;
    type ExecutionPayloadEnvelopeV4 = OpExecutionPayloadEnvelopeV4;

    fn block_to_payload(
        block: SealedBlock<
            <<Self::BuiltPayload as BuiltPayload>::Primitives as NodePrimitives>::Block,
        >,
    ) -> (alloy_rpc_types_engine::ExecutionPayload, alloy_rpc_types_engine::ExecutionPayloadSidecar)
    {
        let hash = block.hash();

        ExecutionPayload::from_block_unchecked(
            hash,
            &block.into_block().map_header(|header| header.inner),
        )
    }
}
