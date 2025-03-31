use crate::{hardforks::BscHardforks, system_contracts::get_upgrade_system_contracts};
use alloy_evm::{block::ExecutableTx, eth::EthBlockExecutor};
use alloy_primitives::Address;
use reth_chainspec::{ChainSpec, EthChainSpec, EthereumHardforks, Hardforks};
use reth_evm::{
    execute::{BlockExecutionError, BlockExecutor},
    Database, Evm, OnStateHook,
};
use reth_evm_ethereum::RethReceiptBuilder;
use reth_primitives::{Receipt, TransactionSigned};
use reth_provider::BlockExecutionResult;
use reth_revm::State;
use revm::{
    context::{result::ExecutionResult, TxEnv},
    state::Bytecode,
};
use std::sync::Arc;

struct BscBlockExecutor<'a, EVM, Spec> {
    /// Inner Ethereum execution strategy.
    inner: EthBlockExecutor<'a, EVM, &'a Arc<ChainSpec>, &'a RethReceiptBuilder>,
    /// Reference to the specification object.
    spec: Spec,
    /// Inner EVM.
    evm: EVM,
}

impl<'a, DB, EVM, Spec> BscBlockExecutor<'a, EVM, Spec>
where
    DB: Database + 'a,
    EVM: Evm<DB = &'a mut State<DB>, Tx = TxEnv>,
    Spec: EthereumHardforks + BscHardforks + EthChainSpec + Hardforks,
{
    /// Creates a new BscBlockExecutor.
    pub fn new(
        inner: EthBlockExecutor<'a, EVM, &'a Arc<ChainSpec>, &'a RethReceiptBuilder>,
        spec: Spec,
        evm: EVM,
    ) -> Self {
        Self { inner, spec, evm }
    }

    /// Applies system contract upgrades if the Feynman fork is not yet active.
    fn apply_upgrade_contracts_if_before_feynman(&mut self) -> Result<(), BlockExecutionError> {
        if self.spec.is_feynman_active_at_timestamp(self.evm.block().timestamp) {
            return Ok(());
        }

        let contracts = get_upgrade_system_contracts(
            &self.spec,
            self.evm.block().number,
            self.evm.block().timestamp,
            self.evm.block().timestamp - 3_000, // TODO: how to get parent block timestamp?
        )
        .map_err(|_| BlockExecutionError::msg("Failed to get upgrade system contracts"))?;

        for (address, maybe_code) in contracts {
            if let Some(code) = maybe_code {
                self.upgrade_system_contract(address, code)?;
            }
        }

        Ok(())
    }

    /// Replaces the code of a system contract in state.
    fn upgrade_system_contract(
        &mut self,
        address: Address,
        code: Bytecode,
    ) -> Result<(), BlockExecutionError> {
        let account =
            self.evm.db_mut().load_cache_account(address).map_err(BlockExecutionError::other)?;

        let mut info = account.account_info().unwrap_or_default();
        info.code_hash = code.hash_slow();
        info.code = Some(code);

        let transition = account.change(info, Default::default());
        self.evm.db_mut().apply_transition(vec![(address, transition)]);
        Ok(())
    }
}

impl<'a, DB, E, Spec> BlockExecutor for BscBlockExecutor<'a, E, Spec>
where
    DB: Database + 'a,
    E: Evm<DB = &'a mut State<DB>, Tx = TxEnv>,
    Spec: EthereumHardforks + BscHardforks + EthChainSpec + Hardforks,
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
        self.apply_upgrade_contracts_if_before_feynman()?;

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
        // Consensus: Verify validators
        // Consensus:Verify turn length
        // If first block init genesis contracts
        // Upgrade system contracts
        // Init feynman contracts
        // Consensus:Slash validator if not in turn
        // Consensus:Distribute rewards
        // Consensus: Update validator set
        todo!()
    }

    fn set_state_hook(&mut self, hook: Option<Box<dyn OnStateHook>>) {
        self.inner.set_state_hook(hook)
    }

    fn evm_mut(&mut self) -> &mut Self::Evm {
        self.inner.evm_mut()
    }
}
