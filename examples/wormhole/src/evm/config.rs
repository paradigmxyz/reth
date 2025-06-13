use crate::evm::{alloy::WormholeEvmFactory, WormholeBlockAssembler};
use alloy_consensus::BlockHeader;
use alloy_evm::EvmEnv;
use alloy_op_evm::OpBlockExecutionCtx;
use op_revm::OpSpecId;
use reth_node_api::ConfigureEvm;
use reth_op::{
    chainspec::OpChainSpec,
    node::{OpEvmConfig, OpNextBlockEnvAttributes, OpRethReceiptBuilder},
};
use reth_primitives_traits::{SealedBlock, SealedHeader};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct WormholeEvmConfig {
    pub(super) inner: OpEvmConfig,
    pub(super) block_assembler: WormholeBlockAssembler,
    pub(super) wormhole_evm_factory: WormholeEvmFactory,
}

impl WormholeEvmConfig {
    pub fn new(chain_spec: Arc<OpChainSpec>) -> Self {
        Self {
            inner: OpEvmConfig::new(chain_spec.clone(), OpRethReceiptBuilder::default()),
            block_assembler: WormholeBlockAssembler::new(chain_spec),
            wormhole_evm_factory: WormholeEvmFactory::new(),
        }
    }
}

impl ConfigureEvm for WormholeEvmConfig {
    type Primitives = reth_op::OpPrimitives;
    type Error = <OpEvmConfig as ConfigureEvm>::Error;
    type NextBlockEnvCtx = <OpEvmConfig as ConfigureEvm>::NextBlockEnvCtx;
    type BlockExecutorFactory = Self;
    type BlockAssembler = WormholeBlockAssembler;

    fn block_executor_factory(&self) -> &Self::BlockExecutorFactory {
        self
    }

    fn block_assembler(&self) -> &Self::BlockAssembler {
        &self.block_assembler
    }

    fn evm_env(&self, header: &alloy_consensus::Header) -> EvmEnv<OpSpecId> {
        self.inner.evm_env(header)
    }

    fn next_evm_env(
        &self,
        parent: &alloy_consensus::Header,
        attributes: &OpNextBlockEnvAttributes,
    ) -> Result<EvmEnv<OpSpecId>, Self::Error> {
        self.inner.next_evm_env(parent, attributes)
    }

    fn context_for_block(
        &self,
        block: &SealedBlock<alloy_consensus::Block<op_alloy_consensus::OpTxEnvelope>>,
    ) -> OpBlockExecutionCtx {
        OpBlockExecutionCtx {
            parent_hash: block.header().parent_hash(),
            parent_beacon_block_root: block.header().parent_beacon_block_root(),
            extra_data: block.header().extra_data().clone(),
        }
    }

    fn context_for_next_block(
        &self,
        parent: &SealedHeader<alloy_consensus::Header>,
        attributes: Self::NextBlockEnvCtx,
    ) -> OpBlockExecutionCtx {
        OpBlockExecutionCtx {
            parent_hash: parent.hash(),
            parent_beacon_block_root: attributes.parent_beacon_block_root,
            extra_data: attributes.extra_data,
        }
    }
}
