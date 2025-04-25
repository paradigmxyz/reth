use crate::primitives::CustomNodePrimitives;
use op_alloy_rpc_types_engine::{OpExecutionData, OpExecutionPayload};
use reth_chain_state::ExecutedBlockWithTrieUpdates;
use reth_ethereum::{
    node::api::{
        BuiltPayload, ExecutionPayload, NodePrimitives, PayloadAttributes,
        PayloadBuilderAttributes, PayloadTypes,
    },
    primitives::SealedBlock,
};
use reth_op::{
    node::{OpBuiltPayload, OpPayloadAttributes, OpPayloadBuilderAttributes},
    OpTransactionSigned,
};
use revm_primitives::U256;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct CustomPayloadTypes;

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

impl PayloadTypes for CustomPayloadTypes {
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
