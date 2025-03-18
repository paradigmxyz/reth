/*
use crate::{
    chainspec::CustomChainSpec,
    primitives::{Block, CustomHeader, CustomNodePrimitives},
};
use alloy_consensus::{
    constants::EMPTY_WITHDRAWALS, proofs, Header, TxReceipt, EMPTY_OMMER_ROOT_HASH,
};
use alloy_eips::merge::BEACON_NONCE;
use alloy_evm::block::BlockExecutorFor;
use alloy_op_evm::{OpBlockExecutionCtx, OpBlockExecutor, OpEvm, OpEvmFactory};
use op_revm::OpSpecId;
use reth_chainspec::EthChainSpec;
use reth_evm::{
    block::BlockExecutionError,
    execute::{BlockAssembler, BlockAssemblerInput, BlockExecutorFactory},
    ConfigureEvm, Database, EvmEnv, InspectorFor,
};
use reth_execution_types::BlockExecutionResult;
use reth_optimism_consensus::{calculate_receipt_root_no_memo_optimism, isthmus};
use reth_optimism_forks::OpHardforks;
use reth_optimism_node::{
    OpBlockAssembler, OpEvmConfig, OpNextBlockEnvAttributes, OpRethReceiptBuilder,
};
use reth_optimism_primitives::{OpReceipt, OpTransactionSigned};
use reth_primitives::{logs_bloom, BlockBody, SealedBlock};
use reth_primitives_traits::SealedHeader;
use reth_revm::State;
use std::sync::Arc;


#[derive(Debug, Clone)]
pub struct CustomBlockAssembler<CS> {
    inner: OpBlockAssembler<CS>,
    chain_spec: Arc<CS>,
}

impl<CS> BlockAssembler<CustomEvmConfig> for CustomBlockAssembler<CS>
where
    CS: EthChainSpec + OpHardforks + Sync + Send + 'static,
{
    type Block = Block;

    fn assemble_block(
        &self,
        input: BlockAssemblerInput<'_, '_, CustomEvmConfig, CustomHeader>,
    ) -> Result<Self::Block, BlockExecutionError> {
        let BlockAssemblerInput {
            evm_env,
            execution_ctx: ctx,
            transactions,
            output: BlockExecutionResult { receipts, gas_used, .. },
            bundle_state,
            state_root,
            state_provider,
            ..
        } = input;

        let timestamp = evm_env.block_env.timestamp;

        let transactions_root = proofs::calculate_transaction_root(&transactions);
        let receipts_root =
            calculate_receipt_root_no_memo_optimism(receipts, &self.chain_spec, timestamp);
        let logs_bloom = logs_bloom(receipts.iter().flat_map(|r| r.logs()));

        let withdrawals_root = if self.chain_spec.is_isthmus_active_at_timestamp(timestamp) {
            // withdrawals root field in block header is used for storage root of L2 predeploy
            // `l2tol1-message-passer`
            Some(
                isthmus::withdrawals_root(bundle_state, state_provider)
                    .map_err(BlockExecutionError::other)?,
            )
        } else if self.chain_spec.is_canyon_active_at_timestamp(timestamp) {
            Some(EMPTY_WITHDRAWALS)
        } else {
            None
        };

        let (excess_blob_gas, blob_gas_used) =
            if self.chain_spec.is_ecotone_active_at_timestamp(timestamp) {
                (Some(0), Some(0))
            } else {
                (None, None)
            };

        let inner_header = Header {
            parent_hash: ctx.parent_hash,
            ommers_hash: EMPTY_OMMER_ROOT_HASH,
            beneficiary: evm_env.block_env.beneficiary,
            state_root,
            transactions_root,
            receipts_root,
            withdrawals_root,
            logs_bloom,
            timestamp,
            mix_hash: evm_env.block_env.prevrandao.unwrap_or_default(),
            nonce: BEACON_NONCE.into(),
            base_fee_per_gas: Some(evm_env.block_env.basefee),
            number: evm_env.block_env.number,
            gas_limit: evm_env.block_env.gas_limit,
            difficulty: evm_env.block_env.difficulty,
            gas_used: *gas_used,
            extra_data: ctx.extra_data,
            parent_beacon_block_root: ctx.parent_beacon_block_root,
            blob_gas_used,
            excess_blob_gas,
            requests_hash: None,
        };

        let header = CustomHeader { inner: inner_header, extension: 0 };

        Ok(Block::new(
            header,
            BlockBody {
                transactions,
                ommers: Default::default(),
                withdrawals: self
                    .chain_spec
                    .is_canyon_active_at_timestamp(timestamp)
                    .then(Default::default),
            },
        ))
    }
}

#[derive(Debug, Clone)]
pub struct CustomEvmConfig {
    inner: OpEvmConfig<CustomChainSpec, CustomNodePrimitives>,
    block_assembler: CustomBlockAssembler<CustomChainSpec>,
}

impl CustomEvmConfig {
    pub fn new(chain_spec: Arc<CustomChainSpec>) -> Self {
        // Specify CustomNodePrimitives explicitly when creating OpEvmConfig
        let inner: OpEvmConfig<CustomChainSpec, CustomNodePrimitives> =
            OpEvmConfig::new(chain_spec.clone(), OpRethReceiptBuilder::default());
        Self {
            inner,
            block_assembler: CustomBlockAssembler {
                inner: OpBlockAssembler::new(chain_spec.clone()),
                chain_spec,
            },
        }
    }
}

impl BlockExecutorFactory for CustomEvmConfig {
    type EvmFactory = OpEvmFactory;
    type ExecutionCtx<'a> = OpBlockExecutionCtx;
    type Transaction = OpTransactionSigned;
    type Receipt = OpReceipt;

    fn evm_factory(&self) -> &Self::EvmFactory {
        self.inner.evm_factory()
    }

    fn create_executor<'a, DB, I>(
        &'a self,
        evm: OpEvm<&'a mut State<DB>, I>,
        ctx: OpBlockExecutionCtx,
    ) -> impl BlockExecutorFor<'a, Self, DB, I>
    where
        DB: Database + 'a,
        I: InspectorFor<Self, &'a mut State<DB>> + 'a,
    {
        OpBlockExecutor::new(
            evm,
            ctx,
            self.inner.chain_spec(),
            self.inner.executor_factory.receipt_builder(),
        )
    }
}

impl ConfigureEvm for CustomEvmConfig {
    type Primitives = CustomNodePrimitives;
    type Error = <OpEvmConfig as ConfigureEvm>::Error;
    type NextBlockEnvCtx = <OpEvmConfig as ConfigureEvm>::NextBlockEnvCtx;
    type BlockExecutorFactory = Self;
    type BlockAssembler = CustomBlockAssembler<CustomChainSpec>;

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

    fn context_for_block<'a>(&self, block: &'a SealedBlock<Block>) -> OpBlockExecutionCtx {
        self.inner.context_for_block(block)
    }

    fn context_for_next_block(
        &self,
        parent: &SealedHeader<CustomHeader>,
        attributes: Self::NextBlockEnvCtx,
    ) -> OpBlockExecutionCtx {
        self.inner.context_for_next_block(parent, attributes)
    }
}
*/
