use alloy_evm::{block::ExecutableTx, eth::EthBlockExecutor};
use reth_chainspec::{ChainSpec, EthereumHardforks};
use reth_evm::{
    execute::{BlockExecutionError, BlockExecutor},
    Database, Evm, OnStateHook,
};
use reth_evm_ethereum::RethReceiptBuilder;
use reth_primitives::{Receipt, TransactionSigned};
use reth_provider::BlockExecutionResult;
use reth_revm::State;
use revm::context::{result::ExecutionResult, TxEnv};
use std::sync::Arc;

struct BscBlockExecutor<'a, Evm, Spec> {
    /// Inner Ethereum execution strategy.
    inner: EthBlockExecutor<'a, Evm, &'a Arc<ChainSpec>, &'a RethReceiptBuilder>,
    /// Reference to the specification object.
    spec: Spec,
    /// Inner EVM.
    evm: Evm,
}

impl<'a, Evm, Spec> BscBlockExecutor<'a, Evm, Spec> {
    /// Creates a new BscBlockExecutor.
    pub fn new(
        inner: EthBlockExecutor<'a, Evm, &'a Arc<ChainSpec>, &'a RethReceiptBuilder>,
        spec: Spec,
        evm: Evm,
    ) -> Self {
        Self { inner, spec, evm }
    }
}

impl<'db, DB, E, Spec> BlockExecutor for BscBlockExecutor<'_, E, Spec>
where
    DB: Database + 'db,
    E: Evm<DB = &'db mut State<DB>, Tx = TxEnv>,
    Spec: EthereumHardforks,
{
    type Transaction = TransactionSigned;
    type Receipt = Receipt;
    type Evm = E;

    fn apply_pre_execution_changes(&mut self) -> Result<(), BlockExecutionError> {
        // Set state clear flag if the block is after the Spurious Dragon hardfork.
        let state_clear_flag =
            self.spec.is_spurious_dragon_active_at_block(self.evm.block().number);
        self.evm.db_mut().set_state_clear_flag(state_clear_flag);
        // TODO: (Consensus Verify cascading fields)[https://github.com/bnb-chain/reth/blob/main/crates/bsc/evm/src/pre_execution.rs#L43]
        // TODO: (Consensus System Call Before Execution)[https://github.com/bnb-chain/reth/blob/main/crates/bsc/evm/src/execute.rs#L678]
        // TODO: (Upgrade system contracts)[https://github.com/bnb-chain/reth/blob/main/crates/bsc/evm/src/execute.rs#L498]
        Ok(())
    }

    fn execute_transaction_with_result_closure(
        &mut self,
        tx: impl ExecutableTx<Self>,
        f: impl FnOnce(&ExecutionResult<<Self::Evm as Evm>::HaltReason>),
    ) -> Result<u64, BlockExecutionError> {
        // check if its system tx and keep it
        // apply patches before
        // execute tx
        // apply patches after
        todo!()
    }

    fn finish(self) -> Result<(Self::Evm, BlockExecutionResult<Receipt>), BlockExecutionError> {
        // Verify validators
        // Verify turn length
        // If first block init genesis contracts
        // Upgrade system contracts
        // Init feynman contracts
        // Slash validator if not in turn
        // Distribute rewards
        // Update validator set
        todo!()
    }

    fn set_state_hook(&mut self, hook: Option<Box<dyn OnStateHook>>) {
        self.inner.set_state_hook(hook)
    }

    fn evm_mut(&mut self) -> &mut Self::Evm {
        self.inner.evm_mut()
    }
}
