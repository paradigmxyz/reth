//! Example for how to modify a block post-execution step. It credits beacon withdrawals with a
//! custom mechanism instead of minting native tokens

#![warn(unused_crate_dependencies)]

use alloy_eips::eip4895::Withdrawal;
use alloy_sol_macro::sol;
use alloy_sol_types::SolCall;
use reth::{
    api::{ConfigureEvm, NodeTypesWithEngine},
    builder::{components::ExecutorBuilder, BuilderContext, FullNodeTypes},
    cli::Cli,
    providers::BlockExecutionResult,
    revm::{
        context::result::ExecutionResult,
        db::State,
        primitives::{address, Address},
        DatabaseCommit,
    },
};
use reth_chainspec::ChainSpec;
use reth_evm::{
    execute::{
        BlockExecutionError, BlockExecutionStrategy, BlockExecutionStrategyFactory,
        InternalBlockExecutionError,
    },
    ConfigureEvmEnv, Database, Evm, EvmEnv, EvmFor, FromRecoveredTx, InspectorFor,
    NextBlockEnvAttributes,
};
use reth_evm_ethereum::{
    execute::{EthBlockExecutionCtx, EthExecutionStrategy},
    EthBlockAssembler, EthEvmConfig,
};
use reth_node_ethereum::{node::EthereumAddOns, BasicBlockExecutorProvider, EthereumNode};
use reth_primitives::{
    EthPrimitives, Receipt, Recovered, SealedBlock, SealedHeader, TransactionSigned,
};
use std::fmt::Display;

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

impl ConfigureEvmEnv for CustomEvmConfig {
    type Error = <EthEvmConfig as ConfigureEvmEnv>::Error;
    type Header = <EthEvmConfig as ConfigureEvmEnv>::Header;
    type Spec = <EthEvmConfig as ConfigureEvmEnv>::Spec;
    type Transaction = <EthEvmConfig as ConfigureEvmEnv>::Transaction;
    type TxEnv = <EthEvmConfig as ConfigureEvmEnv>::TxEnv;
    type NextBlockEnvCtx = NextBlockEnvAttributes;

    fn evm_env(&self, header: &Self::Header) -> EvmEnv<Self::Spec> {
        self.inner.evm_env(header)
    }

    fn next_evm_env(
        &self,
        parent: &Self::Header,
        attributes: &NextBlockEnvAttributes,
    ) -> Result<EvmEnv<Self::Spec>, Self::Error> {
        self.inner.next_evm_env(parent, attributes)
    }
}

impl ConfigureEvm for CustomEvmConfig {
    type EvmFactory = <EthEvmConfig as ConfigureEvm>::EvmFactory;

    fn evm_factory(&self) -> &Self::EvmFactory {
        self.inner.evm_factory()
    }
}

impl BlockExecutionStrategyFactory for CustomEvmConfig {
    type Primitives = EthPrimitives;
    type ExecutionCtx<'a> = EthBlockExecutionCtx<'a>;
    type Strategy<'a, DB: Database + 'a, I: InspectorFor<&'a mut State<DB>, Self> + 'a> =
        CustomExecutorStrategy<'a, EvmFor<Self, &'a mut State<DB>, I>>;
    type BlockAssembler = EthBlockAssembler<ChainSpec>;

    fn block_assembler(&self) -> &Self::BlockAssembler {
        self.inner.block_assembler()
    }

    fn context_for_block<'a>(&self, block: &'a SealedBlock) -> Self::ExecutionCtx<'a> {
        self.inner.context_for_block(block)
    }

    fn context_for_next_block(
        &self,
        parent: &SealedHeader,
        attributes: Self::NextBlockEnvCtx,
    ) -> Self::ExecutionCtx<'_> {
        self.inner.context_for_next_block(parent, attributes)
    }

    fn create_strategy<'a, DB, I>(
        &'a self,
        evm: EvmFor<Self, &'a mut State<DB>, I>,
        ctx: Self::ExecutionCtx<'a>,
    ) -> Self::Strategy<'a, DB, I>
    where
        DB: Database,
        I: InspectorFor<&'a mut State<DB>, Self> + 'a,
    {
        CustomExecutorStrategy { inner: self.inner.create_strategy(evm, ctx) }
    }
}

pub struct CustomExecutorStrategy<'a, Evm> {
    /// Inner Ethereum execution strategy.
    inner: EthExecutionStrategy<'a, Evm>,
}

impl<'db, DB, E> BlockExecutionStrategy for CustomExecutorStrategy<'_, E>
where
    DB: Database + 'db,
    E: Evm<DB = &'db mut State<DB>, Tx: FromRecoveredTx<TransactionSigned>>,
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

    fn with_state_hook(&mut self, _hook: Option<Box<dyn reth_evm::system_calls::OnStateHook>>) {
        self.inner.with_state_hook(_hook)
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
