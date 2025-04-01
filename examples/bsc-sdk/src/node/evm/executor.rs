use super::patch::{patch_mainnet_after_tx, patch_mainnet_before_tx};
use crate::{
    consensus::ParliaConsensus,
    hardforks::BscHardforks,
    system_contracts::{get_upgrade_system_contracts, is_system_transaction},
};
use alloy_consensus::{Transaction, TxReceipt};
use alloy_eips::Encodable2718;
use alloy_evm::{
    block::ExecutableTx,
    eth::{receipt_builder::ReceiptBuilderCtx, EthBlockExecutor},
};
use alloy_primitives::Address;
use reth_chainspec::{ChainSpec, EthChainSpec, EthereumHardforks, Hardforks};
use reth_evm::{
    block::BlockValidationError,
    eth::receipt_builder::ReceiptBuilder,
    execute::{BlockExecutionError, BlockExecutor},
    Database, Evm, FromRecoveredTx, OnStateHook,
};
use reth_evm_ethereum::RethReceiptBuilder;
use reth_primitives::Log;
use reth_primitives_traits::SignedTransaction;
use reth_provider::{BlockExecutionResult, BlockNumReader};
use reth_revm::State;
use revm::{
    context::result::{ExecutionResult, ResultAndState},
    state::Bytecode,
    DatabaseCommit,
};
use std::sync::Arc;

struct BscBlockExecutor<'a, EVM, Spec, R: ReceiptBuilder, P> {
    /// Inner Ethereum execution strategy.
    inner: EthBlockExecutor<'a, EVM, &'a Arc<ChainSpec>, &'a RethReceiptBuilder>,
    /// Reference to the specification object.
    spec: Spec,
    /// Inner EVM.
    evm: EVM,
    /// Gas used in the block.
    gas_used: u64,
    /// Receipts of executed transactions.
    receipts: Vec<R::Receipt>,
    /// System txs
    system_txs: Vec<R::Transaction>,
    /// Receipt builder.
    receipt_builder: R,
    /// Parlia consensus
    consensus: ParliaConsensus<P>,
}

impl<'a, DB, EVM, Spec, R: ReceiptBuilder, P> BscBlockExecutor<'a, EVM, Spec, R, P>
where
    DB: Database + 'a,
    EVM: Evm<DB = &'a mut State<DB>, Tx: FromRecoveredTx<R::Transaction>>,
    Spec: EthereumHardforks + BscHardforks + EthChainSpec + Hardforks,
    P: BlockNumReader + Clone,
{
    /// Creates a new BscBlockExecutor.
    pub fn new(
        inner: EthBlockExecutor<'a, EVM, &'a Arc<ChainSpec>, &'a RethReceiptBuilder>,
        spec: Spec,
        evm: EVM,
        receipt_builder: R,
        consensus: ParliaConsensus<P>,
    ) -> Self {
        Self {
            inner,
            spec,
            evm,
            gas_used: 0,
            receipts: vec![],
            system_txs: vec![],
            receipt_builder,
            consensus,
        }
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

    /// Initializes the feynman contracts
    fn init_feynman_contracts(&self) -> Result<(), BlockExecutionError> {
        let _txs = self.consensus.init_feynman_contracts();
        todo!()
    }

    /// Initializes the genesis contracts
    fn init_genesis_contracts(&self, validator: Address) -> Result<(), BlockExecutionError> {
        let _txs = self.consensus.init_genesis_contracts();
        todo!()
    }

    pub(crate) fn transact_system_tx(
        &self,
        transaction: reth_primitives::Transaction,
        sender: Address,
    ) -> Result<(), BlockExecutionError> {
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

impl<'a, DB, E, Spec, R, P> BlockExecutor for BscBlockExecutor<'a, E, Spec, R, P>
where
    DB: Database + 'a,
    E: Evm<DB = &'a mut State<DB>, Tx: FromRecoveredTx<R::Transaction>>,
    Spec: EthereumHardforks + BscHardforks + EthChainSpec + Hardforks,
    R: ReceiptBuilder<Transaction: SignedTransaction, Receipt: TxReceipt<Log = Log>>,
    <R as ReceiptBuilder>::Transaction: Unpin,
    P: BlockNumReader + Clone,
{
    type Transaction = R::Transaction;
    type Receipt = R::Receipt;
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
        f: impl FnOnce(&ExecutionResult<<<Self as BlockExecutor>::Evm as Evm>::HaltReason>),
    ) -> Result<u64, BlockExecutionError> {
        // Check if it's a system transaction
        let signer = tx.signer();
        if is_system_transaction(tx.tx(), *signer, self.evm.block().beneficiary) {
            self.system_txs.push(tx.tx().clone());
            return Ok(0);
        }

        // apply patches before
        patch_mainnet_before_tx(tx.tx(), self.evm.db_mut())?;

        let block_available_gas = self.evm.block().gas_limit - self.gas_used;
        if tx.tx().gas_limit() > block_available_gas {
            return Err(BlockValidationError::TransactionGasLimitMoreThanAvailableBlockGas {
                transaction_gas_limit: tx.tx().gas_limit(),
                block_available_gas,
            }
            .into());
        }
        let result_and_state = self
            .evm
            .transact(tx)
            .map_err(|err| BlockExecutionError::evm(err, tx.tx().trie_hash()))?;
        let ResultAndState { result, state } = result_and_state;
        f(&result);
        let gas_used = result.gas_used();
        self.gas_used += gas_used;
        self.receipts.push(self.receipt_builder.build_receipt(ReceiptBuilderCtx {
            tx: tx.tx(),
            evm: &self.evm,
            result,
            state: &state,
            cumulative_gas_used: self.gas_used,
        }));
        self.evm.db_mut().commit(state);

        // apply patches after
        patch_mainnet_after_tx(tx.tx(), self.evm.db_mut())?;

        Ok(gas_used)
    }

    fn finish(
        mut self,
    ) -> Result<(Self::Evm, BlockExecutionResult<R::Receipt>), BlockExecutionError> {
        // TODO:
        // Consensus: Verify validators
        // Consensus: Verify turn lengthcons

        // If first block init genesis contracts
        if self.evm.block().number == 1 {
            self.init_genesis_contracts(self.evm.block().beneficiary)?;
        }

        // Upgrade system contracts
        self.apply_upgrade_contracts_if_before_feynman()?;

        // Init feynman contracts
        self.init_feynman_contracts()?;

        // TODO:
        // Consensus: Slash validator if not in turn
        // Consensus: Distribute rewards
        // Consensus: Update validator set
        todo!()
    }

    fn set_state_hook(&mut self, _hook: Option<Box<dyn OnStateHook>>) {}

    fn evm_mut(&mut self) -> &mut Self::Evm {
        &mut self.evm
    }
}
