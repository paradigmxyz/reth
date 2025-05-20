use crate::{
    chainspec::CustomChainSpec,
    primitives::{block::Block, tx, CustomHeader, CustomNodePrimitives, CustomTransaction},
};
use alloy_consensus::Header;
use alloy_eips::Encodable2718;
use alloy_evm::{
    block::{
        BlockExecutionError, BlockExecutionResult, BlockExecutor, BlockExecutorFactory,
        BlockExecutorFor, ExecutableTx, OnStateHook,
    },
    precompiles::PrecompilesMap,
    Database, Evm, EvmEnv, FromRecoveredTx, FromTxWithEncoded,
};
use alloy_op_evm::{
    block::receipt_builder::OpReceiptBuilder, OpBlockExecutionCtx, OpBlockExecutor, OpEvm,
};
use alloy_primitives::{Address, Bytes, TxKind, B256, U256};
use op_alloy_consensus::OpTxEnvelope;
use op_revm::{OpSpecId, OpTransaction};
use reth_chainspec::{ChainSpecProvider, EthChainSpec};
use reth_ethereum::{
    evm::primitives::{
        execute::{BlockAssembler, BlockAssemblerInput},
        InspectorFor,
    },
    node::api::ConfigureEvm,
    primitives::{Receipt, SealedBlock, SealedHeader},
};
use reth_op::{
    chainspec::OpChainSpec,
    node::{
        OpBlockAssembler, OpEvmConfig, OpEvmFactory, OpNextBlockEnvAttributes, OpRethReceiptBuilder,
    },
    DepositReceipt, OpReceipt, OpTransactionSigned,
};
use reth_primitives_traits::NodePrimitives;
use reth_revm::context::TxEnv;
use revm::{context::result::ExecutionResult, database::State};
use std::sync::Arc;

pub struct CustomTxEnv(TxEnv);

impl revm::context::Transaction for CustomTxEnv {
    type AccessListItem<'a>
        = <TxEnv as revm::context::Transaction>::AccessListItem<'a>
    where
        Self: 'a;
    type Authorization<'a>
        = <TxEnv as revm::context::Transaction>::Authorization<'a>
    where
        Self: 'a;

    fn tx_type(&self) -> u8 {
        self.0.tx_type()
    }

    fn caller(&self) -> Address {
        self.0.caller()
    }

    fn gas_limit(&self) -> u64 {
        self.0.gas_limit()
    }

    fn value(&self) -> U256 {
        self.0.value()
    }

    fn input(&self) -> &Bytes {
        self.0.input()
    }

    fn nonce(&self) -> u64 {
        self.0.nonce()
    }

    fn kind(&self) -> TxKind {
        self.0.kind()
    }

    fn chain_id(&self) -> Option<u64> {
        self.0.chain_id()
    }

    fn gas_price(&self) -> u128 {
        self.0.gas_price()
    }

    fn access_list(&self) -> Option<impl Iterator<Item = Self::AccessListItem<'_>>> {
        self.0.access_list()
    }

    fn blob_versioned_hashes(&self) -> &[B256] {
        self.0.blob_versioned_hashes()
    }

    fn max_fee_per_blob_gas(&self) -> u128 {
        self.0.max_fee_per_blob_gas()
    }

    fn authorization_list_len(&self) -> usize {
        self.0.authorization_list_len()
    }

    fn authorization_list(&self) -> impl Iterator<Item = Self::Authorization<'_>> {
        self.0.authorization_list()
    }

    fn max_priority_fee_per_gas(&self) -> Option<u128> {
        self.0.max_priority_fee_per_gas()
    }
}

impl FromRecoveredTx<CustomTransaction> for CustomTxEnv {
    fn from_recovered_tx(tx: &CustomTransaction, sender: Address) -> Self {
        match tx {
            CustomTransaction::BuiltIn(tx) => CustomTxEnv(TxEnv::from_recovered_tx(tx, sender)),
            CustomTransaction::Other(_) => todo!(),
        }
    }
}

impl FromTxWithEncoded<CustomTransaction> for CustomTxEnv {
    fn from_encoded_tx(tx: &CustomTransaction, sender: Address, encoded: Bytes) -> Self {
        match tx {
            CustomTransaction::BuiltIn(tx) => {
                CustomTxEnv(TxEnv::from_encoded_tx(tx, sender, encoded))
            }
            CustomTransaction::Other(_) => todo!(),
        }
    }
}

pub struct CustomEvmTransaction(OpTransaction<CustomTxEnv>);

impl FromRecoveredTx<CustomTransaction> for CustomEvmTransaction {
    fn from_recovered_tx(tx: &CustomTransaction, sender: Address) -> Self {
        match tx {
            CustomTransaction::BuiltIn(tx) => {
                let tx = OpTransaction::<TxEnv>::from_recovered_tx(tx, sender);

                CustomEvmTransaction(OpTransaction {
                    base: CustomTxEnv(tx.base),
                    enveloped_tx: tx.enveloped_tx,
                    deposit: tx.deposit,
                })
            }
            CustomTransaction::Other(_) => todo!(),
        }
    }
}

impl FromTxWithEncoded<CustomTransaction> for CustomEvmTransaction {
    fn from_encoded_tx(tx: &CustomTransaction, sender: Address, encoded: Bytes) -> Self {
        match tx {
            CustomTransaction::BuiltIn(tx) => {
                let tx = OpTransaction::<TxEnv>::from_encoded_tx(tx, sender, encoded.into());

                CustomEvmTransaction(OpTransaction {
                    base: CustomTxEnv(tx.base),
                    enveloped_tx: tx.enveloped_tx,
                    deposit: tx.deposit,
                })
            }
            CustomTransaction::Other(_) => todo!(),
        }
    }
}

pub struct CustomBlockExecutor<Evm> {
    inner: OpBlockExecutor<Evm, OpRethReceiptBuilder, Arc<OpChainSpec>>,
}

impl<'db, DB, E> BlockExecutor for CustomBlockExecutor<E>
where
    DB: Database + 'db,
    E: Evm<DB = &'db mut State<DB>, Tx = CustomEvmTransaction>,
{
    type Transaction = CustomTransaction;
    type Receipt = OpReceipt;
    type Evm = E;

    fn apply_pre_execution_changes(&mut self) -> Result<(), BlockExecutionError> {
        self.inner.apply_pre_execution_changes()
    }

    fn execute_transaction_with_result_closure(
        &mut self,
        tx: impl ExecutableTx<Self>,
        f: impl FnOnce(&ExecutionResult<<Self::Evm as Evm>::HaltReason>),
    ) -> Result<u64, BlockExecutionError> {
        self.inner.execute_transaction_with_result_closure(tx, f)
    }

    fn finish(self) -> Result<(Self::Evm, BlockExecutionResult<OpReceipt>), BlockExecutionError> {
        self.inner.finish()
    }

    fn set_state_hook(&mut self, _hook: Option<Box<dyn OnStateHook>>) {
        self.inner.set_state_hook(_hook)
    }

    fn evm_mut(&mut self) -> &mut Self::Evm {
        self.inner.evm_mut()
    }

    fn evm(&self) -> &Self::Evm {
        self.inner.evm()
    }
}

#[derive(Clone, Debug)]
pub struct CustomBlockAssembler {
    inner: OpBlockAssembler<CustomChainSpec>,
}

impl CustomBlockAssembler {
    pub fn new(inner: OpBlockAssembler<CustomChainSpec>) -> Self {
        Self { inner }
    }
}

impl<F> BlockAssembler<F> for CustomBlockAssembler
where
    F: for<'a> BlockExecutorFactory<
        ExecutionCtx<'a> = OpBlockExecutionCtx,
        Transaction = OpTransactionSigned,
        Receipt: Receipt + DepositReceipt,
    >,
{
    type Block = Block;

    fn assemble_block(
        &self,
        mut input: BlockAssemblerInput<'_, '_, F, CustomHeader>,
    ) -> Result<Self::Block, BlockExecutionError> {
        let inner_input = BlockAssemblerInput {
            evm_env: input.evm_env,
            execution_ctx: input.execution_ctx,
            parent: &input.parent.inner,
            transactions: input.transactions,
            output: input.output,
            bundle_state: input.bundle_state,
            state_provider: input.state_provider,
            state_root: input.state_root,
        };
        let block = self.inner.assemble_block(inner_input)?;
        let block = block.map_transactions(tx::from);
        let block = block.map_header(CustomHeader::from);

        Ok(block)
    }
}

#[derive(Debug, Clone)]
pub struct CustomEvmConfig {
    inner: OpEvmConfig,
    block_assembler: CustomBlockAssembler,
}

impl CustomEvmConfig {
    pub fn new(inner: OpEvmConfig, block_assembler: CustomBlockAssembler) -> Self {
        Self { inner, block_assembler }
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
        evm: OpEvm<&'a mut State<DB>, I, PrecompilesMap>,
        ctx: OpBlockExecutionCtx,
    ) -> impl BlockExecutorFor<'a, Self, DB, I>
    where
        DB: Database + 'a,
        I: InspectorFor<Self, &'a mut State<DB>> + 'a,
    {
        CustomBlockExecutor {
            inner: OpBlockExecutor::new(
                evm,
                ctx,
                self.inner.chain_spec().clone(),
                *self.inner.executor_factory.receipt_builder(),
            ),
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

    fn evm_env(&self, header: &Header) -> EvmEnv<OpSpecId> {
        self.inner.evm_env(header)
    }

    fn next_evm_env(
        &self,
        parent: &Header,
        attributes: &OpNextBlockEnvAttributes,
    ) -> Result<EvmEnv<OpSpecId>, Self::Error> {
        self.inner.next_evm_env(parent, attributes)
    }

    fn context_for_block(
        &self,
        block: &SealedBlock<alloy_consensus::Block<OpTransactionSigned>>,
    ) -> OpBlockExecutionCtx {
        self.inner.context_for_block(block)
    }

    fn context_for_next_block(
        &self,
        parent: &SealedHeader,
        attributes: Self::NextBlockEnvCtx,
    ) -> OpBlockExecutionCtx {
        self.inner.context_for_next_block(parent, attributes)
    }
}
