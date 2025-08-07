use crate::{
    chainspec::CustomChainSpec,
    engine::CustomExecutionData,
    evm::{alloy::CustomEvmFactory, CustomBlockAssembler},
    primitives::{Block, CustomHeader, CustomNodePrimitives, CustomTransaction},
};
use alloy_consensus::BlockHeader;
use alloy_eips::{eip2718::WithEncoded, Decodable2718};
use alloy_evm::EvmEnv;
use alloy_op_evm::OpBlockExecutionCtx;
use alloy_rpc_types_engine::PayloadError;
use op_revm::OpSpecId;
use reth_engine_primitives::ExecutableTxIterator;
use reth_ethereum::{
    node::api::ConfigureEvm,
    primitives::{SealedBlock, SealedHeader},
};
use reth_node_builder::{ConfigureEngineEvm, NewPayloadError};
use reth_op::{
    evm::primitives::{EvmEnvFor, ExecutionCtxFor},
    node::{OpEvmConfig, OpNextBlockEnvAttributes, OpRethReceiptBuilder},
    primitives::SignedTransaction,
};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct CustomEvmConfig {
    pub(super) inner: OpEvmConfig,
    pub(super) block_assembler: CustomBlockAssembler,
    pub(super) custom_evm_factory: CustomEvmFactory,
}

impl CustomEvmConfig {
    pub fn new(chain_spec: Arc<CustomChainSpec>) -> Self {
        Self {
            inner: OpEvmConfig::new(
                Arc::new(chain_spec.inner().clone()),
                OpRethReceiptBuilder::default(),
            ),
            block_assembler: CustomBlockAssembler::new(chain_spec),
            custom_evm_factory: CustomEvmFactory::new(),
        }
    }
}

impl ConfigureEvm for CustomEvmConfig {
    type Primitives = CustomNodePrimitives;
    type Error = <OpEvmConfig as ConfigureEvm>::Error;
    type NextBlockEnvCtx = <OpEvmConfig as ConfigureEvm>::NextBlockEnvCtx;
    type BlockExecutorFactory = Self;
    type BlockAssembler = CustomBlockAssembler;

    fn block_executor_factory(&self) -> &Self::BlockExecutorFactory {
        self
    }

    fn block_assembler(&self) -> &Self::BlockAssembler {
        &self.block_assembler
    }

    fn evm_env(&self, header: &CustomHeader) -> EvmEnv<OpSpecId> {
        self.inner.evm_env(header)
    }

    fn next_evm_env(
        &self,
        parent: &CustomHeader,
        attributes: &OpNextBlockEnvAttributes,
    ) -> Result<EvmEnv<OpSpecId>, Self::Error> {
        self.inner.next_evm_env(parent, attributes)
    }

    fn context_for_block(&self, block: &SealedBlock<Block>) -> OpBlockExecutionCtx {
        OpBlockExecutionCtx {
            parent_hash: block.header().parent_hash(),
            parent_beacon_block_root: block.header().parent_beacon_block_root(),
            extra_data: block.header().extra_data().clone(),
        }
    }

    fn context_for_next_block(
        &self,
        parent: &SealedHeader<CustomHeader>,
        attributes: Self::NextBlockEnvCtx,
    ) -> OpBlockExecutionCtx {
        OpBlockExecutionCtx {
            parent_hash: parent.hash(),
            parent_beacon_block_root: attributes.parent_beacon_block_root,
            extra_data: attributes.extra_data,
        }
    }
}

impl ConfigureEngineEvm<CustomExecutionData> for CustomEvmConfig {
    fn evm_env_for_payload(&self, payload: &CustomExecutionData) -> EvmEnvFor<Self> {
        self.inner.evm_env_for_payload(&payload.inner)
    }

    fn context_for_payload<'a>(
        &self,
        payload: &'a CustomExecutionData,
    ) -> ExecutionCtxFor<'a, Self> {
        self.inner.context_for_payload(&payload.inner)
    }

    fn tx_iterator_for_payload(
        &self,
        payload: &CustomExecutionData,
    ) -> impl ExecutableTxIterator<Self> {
        payload.inner.payload.transactions().clone().into_iter().map(|encoded| {
            let tx = CustomTransaction::decode_2718_exact(encoded.as_ref())
                .map_err(Into::into)
                .map_err(PayloadError::Decode)?;
            let signer = tx.try_recover().map_err(NewPayloadError::other)?;
            Ok::<_, NewPayloadError>(WithEncoded::new(encoded, tx.with_signer(signer)))
        })
    }
}
