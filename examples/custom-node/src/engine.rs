use crate::primitives::CustomNodePrimitives;
use alloy_rpc_types_engine::{
    BlobsBundleV1, ExecutionPayloadV1, ExecutionPayloadV2, ExecutionPayloadV3,
};
use op_alloy_rpc_types_engine::{
    OpExecutionData, OpExecutionPayload, OpExecutionPayloadEnvelopeV3,
    OpExecutionPayloadEnvelopeV4, OpExecutionPayloadV4,
};
use reth_chain_state::ExecutedBlockWithTrieUpdates;
use reth_node_api::{
    BuiltPayload, EngineTypes, ExecutionPayload, NodePrimitives, PayloadAttributes,
    PayloadBuilderAttributes, PayloadTypes,
};
use reth_optimism_node::{OpBuiltPayload, OpPayloadAttributes, OpPayloadBuilderAttributes};
use reth_optimism_primitives::OpTransactionSigned;
use reth_primitives_traits::SealedBlock;
use revm_primitives::U256;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct CustomEngineTypes;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomExecutionData {
    inner: OpExecutionData,
    extension: u64,
}

impl ExecutionPayload for CustomExecutionData {
    fn block_hash(&self) -> revm_primitives::B256 {
        self.inner.block_hash()
    }

    fn block_number(&self) -> u64 {
        self.inner.block_number()
    }

    fn parent_hash(&self) -> revm_primitives::B256 {
        self.inner.parent_hash()
    }

    fn gas_used(&self) -> u64 {
        self.inner.gas_used()
    }

    fn timestamp(&self) -> u64 {
        self.inner.timestamp()
    }

    fn parent_beacon_block_root(&self) -> Option<revm_primitives::B256> {
        self.inner.parent_beacon_block_root()
    }

    fn withdrawals(&self) -> Option<&Vec<alloy_eips::eip4895::Withdrawal>> {
        None
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomPayloadAttributes {
    #[serde(flatten)]
    inner: OpPayloadAttributes,
    extension: u64,
}

impl PayloadAttributes for CustomPayloadAttributes {
    fn parent_beacon_block_root(&self) -> Option<revm_primitives::B256> {
        self.inner.parent_beacon_block_root()
    }

    fn timestamp(&self) -> u64 {
        self.inner.timestamp()
    }

    fn withdrawals(&self) -> Option<&Vec<alloy_eips::eip4895::Withdrawal>> {
        self.inner.withdrawals()
    }
}

#[derive(Debug, Clone)]
pub struct CustomPayloadBuilderAttributes {
    inner: OpPayloadBuilderAttributes<OpTransactionSigned>,
    _extension: u64,
}

impl PayloadBuilderAttributes for CustomPayloadBuilderAttributes {
    type RpcPayloadAttributes = CustomPayloadAttributes;
    type Error = alloy_rlp::Error;

    fn try_new(
        parent: revm_primitives::B256,
        rpc_payload_attributes: Self::RpcPayloadAttributes,
        version: u8,
    ) -> Result<Self, Self::Error>
    where
        Self: Sized,
    {
        let CustomPayloadAttributes { inner, extension } = rpc_payload_attributes;

        Ok(Self {
            inner: OpPayloadBuilderAttributes::try_new(parent, inner, version)?,
            _extension: extension,
        })
    }

    fn parent(&self) -> revm_primitives::B256 {
        self.inner.parent()
    }

    fn parent_beacon_block_root(&self) -> Option<revm_primitives::B256> {
        self.inner.parent_beacon_block_root()
    }

    fn payload_id(&self) -> alloy_rpc_types_engine::PayloadId {
        self.inner.payload_id()
    }

    fn prev_randao(&self) -> revm_primitives::B256 {
        self.inner.prev_randao()
    }

    fn suggested_fee_recipient(&self) -> revm_primitives::Address {
        self.inner.suggested_fee_recipient()
    }

    fn timestamp(&self) -> u64 {
        self.inner.timestamp()
    }

    fn withdrawals(&self) -> &alloy_eips::eip4895::Withdrawals {
        self.inner.withdrawals()
    }
}

#[derive(Debug, Clone)]
pub struct CustomBuiltPayload(pub OpBuiltPayload<CustomNodePrimitives>);

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
        let fees = value.0.fees();
        let block = value.0.into_sealed_block();

        let parent_beacon_block_root = block.parent_beacon_block_root.unwrap_or_default();

        let l2_withdrawals_root = block.withdrawals_root.unwrap_or_default();
        let payload_v3 = ExecutionPayloadV3::from_block_unchecked(
            block.hash(),
            &Arc::unwrap_or_clone(block.into()).into_block(),
        );

        Self {
            execution_payload: OpExecutionPayloadV4::from_v3_with_withdrawals_root(
                payload_v3,
                l2_withdrawals_root,
            ),
            block_value: fees,
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
            parent_beacon_block_root,
            execution_requests: vec![],
        }
    }
}

impl PayloadTypes for CustomEngineTypes {
    type BuiltPayload = CustomBuiltPayload;
    type PayloadAttributes = CustomPayloadAttributes;
    type PayloadBuilderAttributes = CustomPayloadBuilderAttributes;
    type ExecutionData = CustomExecutionData;

    fn block_to_payload(
        block: SealedBlock<
            <<Self::BuiltPayload as BuiltPayload>::Primitives as NodePrimitives>::Block,
        >,
    ) -> Self::ExecutionData {
        let extension = block.header().extension;
        let block_hash = block.hash();
        let block = block.into_block().map_header(|header| header.inner);
        let (payload, sidecar) = OpExecutionPayload::from_block_unchecked(block_hash, &block);
        CustomExecutionData { inner: OpExecutionData { payload, sidecar }, extension }
    }
}

impl EngineTypes for CustomEngineTypes {
    type ExecutionPayloadEnvelopeV1 = ExecutionPayloadV1;
    type ExecutionPayloadEnvelopeV2 = ExecutionPayloadV2;
    type ExecutionPayloadEnvelopeV3 = OpExecutionPayloadEnvelopeV3;
    type ExecutionPayloadEnvelopeV4 = OpExecutionPayloadEnvelopeV4;
}
