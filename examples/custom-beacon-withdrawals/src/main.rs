//! Example for how to modify a block post-execution step. It credits beacon withdrawals with a
//! custom mechanism instead of minting native tokens

#![warn(unused_crate_dependencies)]

use alloy_eips::eip4895::Withdrawal;
use alloy_primitives::{address, Address};
use alloy_sol_types::{sol, SolCall};
use reth_ethereum::{
    chainspec::ChainSpec,
    cli::interface::Cli,
    evm::{
        primitives::{
            block::StateDB, Evm, EvmEnv, EvmEnvFor, ExecutionCtxFor, InspectorFor,
            NextBlockEnvAttributes,
        },
        EthBlockAssembler, EthEvm2Config,
    },
    node::{
        api::{ConfigureEngineEvm, ConfigureEvm, ExecutableTxIterator, FullNodeTypes, NodeTypes},
        builder::{components::ExecutorBuilder, BuilderContext},
        node::EthereumAddOns,
        EthereumNode,
    },
    primitives::{Header, SealedBlock, SealedHeader},
    rpc::types::engine::ExecutionData,
    Block, EthPrimitives, Receipt, TransactionSigned,
};
use reth_evm::{
    block::{
        BlockExecutionError, BlockExecutionResult, BlockExecutor, BlockExecutorFactory,
        ExecutableTx, GasOutput, InternalBlockExecutionError,
    },
    context::{Block as _, BlockEnv, EVMError, HaltReason, TxEnv},
    database::DatabaseCommit,
    eth::EthBlockExecutionCtx,
    evm2::{Evm2RethBlockExecutor, Evm2TxExecutionResult, RethEvm2ReceiptBuilder},
    hardfork::SpecId,
    precompiles::PrecompilesMap,
    EthEvm, EthEvmFactory, FromRecoveredTx, FromTxWithEncoded,
};
use std::fmt::Display;

pub const SYSTEM_ADDRESS: Address = address!("0xfffffffffffffffffffffffffffffffffffffffe");
pub const WITHDRAWALS_ADDRESS: Address = address!("0x4200000000000000000000000000000000000000");

fn main() {
    Cli::parse_args()
        .run(async move |builder, _| {
            let handle = builder
                // use the default ethereum node types
                .with_types::<EthereumNode>()
                // Configure the components of the node
                // use default ethereum components but use our custom pool
                .with_components(
                    EthereumNode::components().executor(CustomExecutorBuilder::default()),
                )
                .with_add_ons(EthereumAddOns::default())
                .launch()
                .await?;

            handle.wait_for_node_exit().await
        })
        .unwrap();
}

/// A custom executor builder
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct CustomExecutorBuilder;

impl<Types, Node> ExecutorBuilder<Node> for CustomExecutorBuilder
where
    Types: NodeTypes<ChainSpec = ChainSpec, Primitives = EthPrimitives>,
    Node: FullNodeTypes<Types = Types>,
{
    type EVM = CustomEvmConfig;

    async fn build_evm(self, ctx: &BuilderContext<Node>) -> eyre::Result<Self::EVM> {
        let evm_config = CustomEvmConfig { inner: EthEvm2Config::new(ctx.chain_spec()) };

        Ok(evm_config)
    }
}

#[derive(Debug, Clone)]
pub struct CustomEvmConfig {
    inner: EthEvm2Config,
}

impl BlockExecutorFactory for CustomEvmConfig {
    type EvmFactory = EthEvmFactory;
    type ExecutionCtx<'a> = EthBlockExecutionCtx<'a>;
    type Transaction = TransactionSigned;
    type Receipt = Receipt;
    type TxExecutionResult = Evm2TxExecutionResult;
    type Executor<'a, DB: StateDB, I: InspectorFor<Self, DB>> =
        CustomBlockExecutor<'a, DB, I>;

    fn evm_factory(&self) -> &Self::EvmFactory {
        self.inner.block_executor_factory().evm_factory()
    }

    fn create_executor<'a, DB, I>(
        &'a self,
        evm: EthEvm<DB, I, PrecompilesMap>,
        ctx: EthBlockExecutionCtx<'a>,
    ) -> Self::Executor<'a, DB, I>
    where
        DB: StateDB,
        I: InspectorFor<Self, DB>,
    {
        CustomBlockExecutor { inner: self.inner.executor_factory.create_executor(evm, ctx) }
    }
}

impl ConfigureEvm for CustomEvmConfig {
    type Primitives = <EthEvm2Config as ConfigureEvm>::Primitives;
    type Error = <EthEvm2Config as ConfigureEvm>::Error;
    type NextBlockEnvCtx = <EthEvm2Config as ConfigureEvm>::NextBlockEnvCtx;
    type BlockExecutorFactory = Self;
    type BlockAssembler = EthBlockAssembler<ChainSpec>;

    fn block_executor_factory(&self) -> &Self::BlockExecutorFactory {
        self
    }

    fn block_assembler(&self) -> &Self::BlockAssembler {
        self.inner.block_assembler()
    }

    fn evm_env(&self, header: &Header) -> Result<EvmEnv<SpecId>, Self::Error> {
        self.inner.evm_env(header)
    }

    fn precompiles(&self, header: &Header) -> Result<Vec<(String, Address)>, Self::Error> {
        self.inner.precompiles(header)
    }

    fn next_evm_env(
        &self,
        parent: &Header,
        attributes: &NextBlockEnvAttributes,
    ) -> Result<EvmEnv<SpecId>, Self::Error> {
        self.inner.next_evm_env(parent, attributes)
    }

    fn context_for_block<'a>(
        &self,
        block: &'a SealedBlock<Block>,
    ) -> Result<EthBlockExecutionCtx<'a>, Self::Error> {
        self.inner.context_for_block(block)
    }

    fn context_for_next_block(
        &self,
        parent: &SealedHeader,
        attributes: Self::NextBlockEnvCtx,
    ) -> Result<EthBlockExecutionCtx<'_>, Self::Error> {
        self.inner.context_for_next_block(parent, attributes)
    }
}

impl ConfigureEngineEvm<ExecutionData> for CustomEvmConfig {
    fn evm_env_for_payload(&self, payload: &ExecutionData) -> Result<EvmEnvFor<Self>, Self::Error> {
        self.inner.evm_env_for_payload(payload)
    }

    fn context_for_payload<'a>(
        &self,
        payload: &'a ExecutionData,
    ) -> Result<ExecutionCtxFor<'a, Self>, Self::Error> {
        self.inner.context_for_payload(payload)
    }

    fn tx_iterator_for_payload(
        &self,
        payload: &ExecutionData,
    ) -> Result<impl ExecutableTxIterator<Self>, Self::Error> {
        self.inner.tx_iterator_for_payload(payload)
    }
}

pub struct CustomBlockExecutor<'a, DB: StateDB, I> {
    /// Inner Ethereum execution strategy.
    inner: Evm2RethBlockExecutor<'a, EthEvm<DB, I, PrecompilesMap>, RethEvm2ReceiptBuilder>,
}

impl<DB, I> BlockExecutor for CustomBlockExecutor<'_, DB, I>
where
    DB: StateDB,
    I: InspectorFor<CustomEvmConfig, DB>,
    EthEvm<DB, I, PrecompilesMap>: Evm<
        DB = DB,
        Tx = TxEnv,
        HaltReason = HaltReason,
        Error = EVMError<DB::Error>,
        Spec = SpecId,
        BlockEnv = BlockEnv,
    >,
    TxEnv: FromRecoveredTx<TransactionSigned> + FromTxWithEncoded<TransactionSigned>,
{
    type Transaction = TransactionSigned;
    type Receipt = Receipt;
    type Evm = EthEvm<DB, I, PrecompilesMap>;
    type DB = DB;
    type Result = Evm2TxExecutionResult;

    fn apply_pre_execution_changes(&mut self) -> Result<(), BlockExecutionError> {
        self.inner.apply_pre_execution_changes()
    }

    fn receipts(&self) -> &[Self::Receipt] {
        self.inner.receipts()
    }

    fn execute_transaction_without_commit(
        &mut self,
        tx: impl ExecutableTx<Self>,
    ) -> Result<Self::Result, BlockExecutionError> {
        self.inner.execute_transaction_without_commit(tx)
    }

    fn commit_transaction(&mut self, output: Self::Result) -> GasOutput {
        self.inner.commit_transaction(output)
    }

    fn finish(mut self) -> Result<(Self::Evm, BlockExecutionResult<Receipt>), BlockExecutionError> {
        if let Some(withdrawals) = self.inner.ctx_mut().withdrawals.clone() {
            apply_withdrawals_contract_call(withdrawals.as_ref(), self.inner.evm_mut())?;
        }

        // Invoke inner finish method to apply Ethereum post-execution changes
        self.inner.finish()
    }

    fn finish_with_db(
        self,
    ) -> Result<(Self::DB, BlockExecutionResult<Self::Receipt>), BlockExecutionError> {
        self.finish().map(|(evm, result)| (evm.into_db(), result))
    }

    fn evm_mut(&mut self) -> &mut Self::Evm {
        self.inner.evm_mut()
    }

    fn evm(&self) -> &Self::Evm {
        self.inner.evm()
    }

    fn db_mut(&mut self) -> &mut Self::DB {
        self.inner.evm_mut().db_mut()
    }

    fn db(&self) -> &Self::DB {
        self.inner.evm().db()
    }
}

sol!(
    function withdrawals(
        uint64[] calldata amounts,
        address[] calldata addresses
    );
);

/// Applies the post-block call to the withdrawal / deposit contract, using the given block,
/// [`ChainSpec`], EVM.
pub fn apply_withdrawals_contract_call(
    withdrawals: &[Withdrawal],
    evm: &mut impl Evm<Error: Display, DB: DatabaseCommit>,
) -> Result<(), BlockExecutionError> {
    let mut state = match evm.transact_system_call(
        SYSTEM_ADDRESS,
        WITHDRAWALS_ADDRESS,
        withdrawalsCall {
            amounts: withdrawals.iter().map(|w| w.amount).collect::<Vec<_>>(),
            addresses: withdrawals.iter().map(|w| w.address).collect::<Vec<_>>(),
        }
        .abi_encode()
        .into(),
    ) {
        Ok(res) => res.state,
        Err(e) => {
            return Err(BlockExecutionError::Internal(InternalBlockExecutionError::Other(
                format!("withdrawal contract system call revert: {e}").into(),
            )))
        }
    };

    // Clean-up post system tx context
    state.remove(&SYSTEM_ADDRESS);
    state.remove(&evm.block().beneficiary());

    evm.db_mut().commit(state);

    Ok(())
}
