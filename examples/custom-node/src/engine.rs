use std::sync::Arc;

use crate::{chainspec::CustomChainSpec, primitives::CustomNodePrimitives, txpool::CustomTxPool};
use alloy_consensus::{Block, BlockBody};
use alloy_rpc_types_engine::{
    BlobsBundleV1, ExecutionPayload, ExecutionPayloadV1, ExecutionPayloadV2, ExecutionPayloadV3,
};
use op_alloy_rpc_types_engine::{OpExecutionPayloadEnvelopeV3, OpExecutionPayloadEnvelopeV4};
use reth_basic_payload_builder::{BuildArguments, BuildOutcome, PayloadBuilder, PayloadConfig};
use reth_chain_state::{ExecutedBlock, ExecutedBlockWithTrieUpdates};
use reth_chainspec::ChainSpecProvider;
use reth_node_api::{
    BuiltPayload, EngineTypes, ExecutionData, NodePrimitives, PayloadAttributes,
    PayloadBuilderAttributes, PayloadBuilderError, PayloadTypes,
};
use reth_optimism_node::{
    BasicOpReceiptBuilder, OpBuiltPayload, OpEngineTypes, OpEvmConfig, OpPayloadAttributes,
    OpPayloadBuilder, OpPayloadBuilderAttributes,
};
use reth_optimism_primitives::{OpBlock, OpReceipt, OpTransactionSigned};
use reth_primitives_traits::{node::AnyNodePrimitives, RecoveredBlock, SealedBlock};
use reth_storage_api::{BlockReaderIdExt, StateProviderFactory};
use revm_primitives::U256;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy)]
pub struct CustomEngineTypes;

#[derive(Debug, Clone)]
pub struct CustomBuiltPayload(OpBuiltPayload<CustomNodePrimitives>);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomExecutionData {
    inner: ExecutionData,
    extension: u64,
}

impl reth_node_api::ExecutionPayload for CustomExecutionData {
    fn block_hash(&self) -> revm_primitives::B256 {
        self.inner.block_hash()
    }

    fn block_number(&self) -> u64 {
        self.inner.block_number()
    }

    fn parent_hash(&self) -> revm_primitives::B256 {
        self.inner.parent_hash()
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
    extension: u64,
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
            extension: rpc_payload_attributes.extension,
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
    type PayloadAttributes = CustomPayloadAttributes;
    type PayloadBuilderAttributes = CustomPayloadBuilderAttributes;
}

impl EngineTypes for CustomEngineTypes {
    type ExecutionPayloadEnvelopeV1 = ExecutionPayloadV1;
    type ExecutionPayloadEnvelopeV2 = ExecutionPayloadV2;
    type ExecutionPayloadEnvelopeV3 = OpExecutionPayloadEnvelopeV3;
    type ExecutionPayloadEnvelopeV4 = OpExecutionPayloadEnvelopeV4;
    type ExecutionData = CustomExecutionData;

    fn block_to_payload(
        block: SealedBlock<
            <<Self::BuiltPayload as BuiltPayload>::Primitives as NodePrimitives>::Block,
        >,
    ) -> Self::ExecutionData {
        let extension = block.header().extension;
        let block_hash = block.hash();
        let block = block.into_block().map_header(|header| header.inner);
        let (payload, sidecar) = ExecutionPayload::from_block_unchecked(block_hash, &block);
        CustomExecutionData { inner: ExecutionData { payload, sidecar }, extension }
    }
}

/// Helper primitives to pass to the OP payload builder.
type PayloadPrimitives = AnyNodePrimitives<Block<OpTransactionSigned>, OpReceipt>;

impl From<CustomBuiltPayload> for OpBuiltPayload<PayloadPrimitives> {
    fn from(value: CustomBuiltPayload) -> Self {
        let OpBuiltPayload { id, block, fees, executed_block } = value.0;
        let (block, hash) = Arc::unwrap_or_clone(block).split();
        let block =
            Arc::new(SealedBlock::new_unchecked(block.map_header(|header| header.inner), hash));
        let executed_block = executed_block.map(|block| {
            let ExecutedBlockWithTrieUpdates {
                block: ExecutedBlock { recovered_block, execution_output, hashed_state },
                trie,
            } = block;

            let RecoveredBlock { block, senders } = Arc::unwrap_or_clone(recovered_block);
            let (block, hash) = block.split();
            let block = SealedBlock::new_unchecked(block.map_header(|header| header.inner), hash);

            ExecutedBlockWithTrieUpdates {
                block: ExecutedBlock {
                    recovered_block: Arc::new(RecoveredBlock::new_sealed(block, senders)),
                    execution_output,
                    hashed_state,
                },
                trie,
            }
        });

        OpBuiltPayload { id, block, fees, executed_block }
    }
}

#[derive(Debug, Clone)]
pub struct CustomPayloadBuilder<Provider> {
    inner: OpPayloadBuilder<
        CustomTxPool<Provider>,
        Provider,
        OpEvmConfig<CustomChainSpec>,
        PayloadPrimitives,
    >,
}

impl<Provider> CustomPayloadBuilder<Provider> {
    pub fn new(
        pool: CustomTxPool<Provider>,
        provider: Provider,
        chain_spec: Arc<CustomChainSpec>,
    ) -> Self {
        Self {
            inner: OpPayloadBuilder::new(
                pool,
                provider,
                OpEvmConfig::new(chain_spec),
                BasicOpReceiptBuilder::default(),
            ),
        }
    }
}

impl<Provider> PayloadBuilder for CustomPayloadBuilder<Provider>
where
    Provider: Clone
        + StateProviderFactory
        + BlockReaderIdExt
        + ChainSpecProvider<ChainSpec = CustomChainSpec>,
{
    type Attributes = CustomPayloadBuilderAttributes;
    type BuiltPayload = CustomBuiltPayload;

    fn try_build(
        &self,
        args: BuildArguments<Self::Attributes, Self::BuiltPayload>,
    ) -> Result<BuildOutcome<Self::BuiltPayload>, PayloadBuilderError> {
        let BuildArguments {
            cached_reads,
            config:
                PayloadConfig {
                    parent_header,
                    attributes: CustomPayloadAttributes { inner, extension },
                },
            cancel,
            best_payload,
        } = args;

        let inner = self.inner.try_build(BuildArguments {
            cached_reads,
            config: PayloadConfig { parent_header: parent_header.inner.clone(), attributes: inner },
            cancel: cancel.clone(),
            best_payload: best_payload.map(Into::into),
        })?;
    }
}
