//! Example for how to modify a block post-execution step. It credits beacon withdrawals with a
//! custom mechanism instead of minting native tokens

#![warn(unused_crate_dependencies)]

use alloy_eips::eip4895::Withdrawal;
use alloy_evm::{
    block::{BlockExecutorFactory, BlockExecutorFor},
    eth::{EthBlockExecutionCtx, EthBlockExecutor},
    EthEvm, EthEvmFactory,
};
use alloy_sol_macro::sol;
use alloy_sol_types::SolCall;
use reth::{
    api::{ConfigureEvm, NodeTypesWithEngine},
    builder::{components::ExecutorBuilder, BuilderContext, FullNodeTypes},
    cli::Cli,
    providers::BlockExecutionResult,
    revm::{
        context::{result::ExecutionResult, TxEnv},
        db::State,
        primitives::{address, hardfork::SpecId, Address},
        DatabaseCommit,
    },
};
use reth_chainspec::ChainSpec;
use reth_evm::{
    execute::{BlockExecutionError, BlockExecutor, InternalBlockExecutionError},
    Database, Evm, EvmEnv, InspectorFor, NextBlockEnvAttributes, OnStateHook,
};
use reth_evm_ethereum::{EthBlockAssembler, EthEvmConfig, RethReceiptBuilder};
use reth_node_ethereum::{node::EthereumAddOns, BasicBlockExecutorProvider, EthereumNode};
use reth_primitives::{
    EthPrimitives, Header, Receipt, Recovered, SealedBlock, SealedHeader, TransactionSigned,
};
use std::{fmt::Display, sync::Arc};

pub const SYSTEM_ADDRESS: Address = address!("0xfffffffffffffffffffffffffffffffffffffffe");
pub const WITHDRAWALS_ADDRESS: Address = address!("0x4200000000000000000000000000000000000000");

fn main() {
    Cli::parse_args()
        .run(|builder, _| async move {
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
    Types: NodeTypesWithEngine<ChainSpec = ChainSpec, Primitives = EthPrimitives>,
    Node: FullNodeTypes<Types = Types>,
{
    type EVM = CustomEvmConfig;
    type Executor = BasicBlockExecutorProvider<Self::EVM>;

    async fn build_evm(
        self,
        ctx: &BuilderContext<Node>,
    ) -> eyre::Result<(Self::EVM, Self::Executor)> {
        let evm_config = CustomEvmConfig { inner: EthEvmConfig::new(ctx.chain_spec()) };
        let executor = BasicBlockExecutorProvider::new(evm_config.clone());

        Ok((evm_config, executor))
    }
}

#[derive(Debug, Clone)]
pub struct CustomEvmConfig {
    inner: EthEvmConfig,
}

impl BlockExecutorFactory for CustomEvmConfig {
    type EvmFactory = EthEvmFactory;
    type ExecutionCtx<'a> = EthBlockExecutionCtx<'a>;
    type Transaction = TransactionSigned;
    type Receipt = Receipt;

    fn evm_factory(&self) -> &Self::EvmFactory {
        self.inner.evm_factory()
    }

    fn create_executor<'a, DB, I>(
        &'a self,
        evm: EthEvm<&'a mut State<DB>, I>,
        ctx: EthBlockExecutionCtx<'a>,
    ) -> impl BlockExecutorFor<'a, Self, DB, I>
    where
        DB: Database + 'a,
        I: InspectorFor<Self, &'a mut State<DB>> + 'a,
    {
        CustomBlockExecutor {
            inner: EthBlockExecutor::new(
                evm,
                ctx,
                self.inner.chain_spec(),
                self.inner.executor_factory.receipt_builder(),
            ),
        }
    }
}

impl ConfigureEvm for CustomEvmConfig {
    type Primitives = <EthEvmConfig as ConfigureEvm>::Primitives;
    type Error = <EthEvmConfig as ConfigureEvm>::Error;
    type NextBlockEnvCtx = <EthEvmConfig as ConfigureEvm>::NextBlockEnvCtx;
    type BlockExecutorFactory = Self;
    type BlockAssembler = EthBlockAssembler<ChainSpec>;

    fn block_executor_factory(&self) -> &Self::BlockExecutorFactory {
        self
    }

    fn block_assembler(&self) -> &Self::BlockAssembler {
        self.inner.block_assembler()
    }

    fn evm_env(&self, header: &Header) -> EvmEnv<SpecId> {
        self.inner.evm_env(header)
    }

    fn next_evm_env(
        &self,
        parent: &Header,
        attributes: &NextBlockEnvAttributes,
    ) -> Result<EvmEnv<SpecId>, Self::Error> {
        self.inner.next_evm_env(parent, attributes)
    }

    fn context_for_block<'a>(&self, block: &'a SealedBlock) -> EthBlockExecutionCtx<'a> {
        self.inner.context_for_block(block)
    }

    fn context_for_next_block(
        &self,
        parent: &SealedHeader,
        attributes: Self::NextBlockEnvCtx,
    ) -> EthBlockExecutionCtx<'_> {
        self.inner.context_for_next_block(parent, attributes)
    }
}

pub struct CustomBlockExecutor<'a, Evm> {
    /// Inner Ethereum execution strategy.
    inner: EthBlockExecutor<'a, Evm, &'a Arc<ChainSpec>, &'a RethReceiptBuilder>,
}

impl<'db, DB, E> BlockExecutor for CustomBlockExecutor<'_, E>
where
    DB: Database + 'db,
    E: Evm<DB = &'db mut State<DB>, Tx = TxEnv>,
{
    type Transaction = TransactionSigned;
    type Receipt = Receipt;
    type Evm = E;

    fn apply_pre_execution_changes(&mut self) -> Result<(), BlockExecutionError> {
        self.inner.apply_pre_execution_changes()
    }

    fn execute_transaction_with_result_closure(
        &mut self,
        tx: Recovered<&TransactionSigned>,
        f: impl FnOnce(&ExecutionResult<<Self::Evm as Evm>::HaltReason>),
    ) -> Result<u64, BlockExecutionError> {
        self.inner.execute_transaction_with_result_closure(tx, f)
    }

    fn finish(mut self) -> Result<(Self::Evm, BlockExecutionResult<Receipt>), BlockExecutionError> {
        if let Some(withdrawals) = self.inner.ctx.withdrawals.clone() {
            apply_withdrawals_contract_call(withdrawals.as_ref(), self.inner.evm_mut())?;
        }

        // Invoke inner finish method to apply Ethereum post-execution changes
        self.inner.finish()
    }

    fn set_state_hook(&mut self, _hook: Option<Box<dyn OnStateHook>>) {
        self.inner.set_state_hook(_hook)
    }

    fn evm_mut(&mut self) -> &mut Self::Evm {
        self.inner.evm_mut()
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
                format!("withdrawal contract system call revert: {}", e).into(),
            )))
        }
    };

    // Clean-up post system tx context
    state.remove(&SYSTEM_ADDRESS);
    state.remove(&evm.block().beneficiary);

    evm.db_mut().commit(state);

    Ok(())
}
