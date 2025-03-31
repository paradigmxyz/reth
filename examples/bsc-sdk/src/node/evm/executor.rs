use super::patch::{patch_mainnet_after_tx, patch_mainnet_before_tx};
use crate::{
    hardforks::BscHardforks,
    system_contracts::{get_upgrade_system_contracts, is_system_transaction, SystemContract},
};
use alloy_consensus::{Transaction, TxReceipt};
use alloy_eips::{eip7685::Requests, Encodable2718};
use alloy_evm::{block::ExecutableTx, eth::receipt_builder::ReceiptBuilderCtx};
use alloy_primitives::Address;
use reth_chainspec::{EthChainSpec, EthereumHardforks, Hardforks};
use reth_evm::{
    block::BlockValidationError,
    eth::{receipt_builder::ReceiptBuilder, EthBlockExecutionCtx},
    execute::{BlockExecutionError, BlockExecutor},
    state_change::post_block_balance_increments,
    Database, Evm, FromRecoveredTx, OnStateHook,
};
use reth_primitives::{Log, Recovered, TransactionSigned};
use reth_primitives_traits::SignedTransaction;
use reth_provider::BlockExecutionResult;
use reth_revm::State;
use revm::{
    context::result::{ExecutionResult, ResultAndState},
    state::Bytecode,
    Database as RevmDatabase, DatabaseCommit,
};

pub struct BscBlockExecutor<'a, EVM, Spec, R: ReceiptBuilder>
where
    Spec: EthChainSpec,
{
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
    /// System contracts used to trigger fork specific logic.
    system_contracts: SystemContract<Spec>,
    /// Context for block execution.
    pub ctx: EthBlockExecutionCtx<'a>,
}

impl<'a, DB, EVM, Spec, R: ReceiptBuilder> BscBlockExecutor<'a, EVM, Spec, R>
where
    DB: Database + 'a,
    EVM: Evm<DB = &'a mut State<DB>, Tx: FromRecoveredTx<R::Transaction>>,
    Spec: EthereumHardforks + BscHardforks + EthChainSpec + Hardforks + Clone,
    R::Transaction: From<TransactionSigned> + Clone,
{
    /// Creates a new BscBlockExecutor.
    pub fn new(
        evm: EVM,
        ctx: EthBlockExecutionCtx<'a>,
        spec: Spec,
        receipt_builder: R,
        system_contracts: SystemContract<Spec>,
    ) -> Self {
        Self {
            spec,
            evm,
            gas_used: 0,
            receipts: vec![],
            system_txs: vec![],
            receipt_builder,
            system_contracts,
            ctx,
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
    fn deploy_feynman_contracts(
        &mut self,
        beneficiary: Address,
    ) -> Result<(), BlockExecutionError> {
        let txs = self.system_contracts.feynman_contracts_txs();
        for mut tx in txs {
            self.transact_system_tx(&mut tx, beneficiary)?;
        }
        Ok(())
    }

    /// Initializes the genesis contracts
    fn deploy_genesis_contracts(
        &mut self,
        beneficiary: Address,
    ) -> Result<(), BlockExecutionError> {
        let txs = self.system_contracts.genesis_contracts_txs();

        for mut tx in txs {
            self.transact_system_tx(&mut tx, beneficiary)?;
        }
        Ok(())
    }

    pub(crate) fn transact_system_tx(
        &mut self,
        tx: &mut TransactionSigned,
        sender: Address,
    ) -> Result<(), BlockExecutionError> {
        let account = self
            .evm
            .db_mut()
            .basic(sender)
            .map_err(BlockExecutionError::other)?
            .ok_or(BlockExecutionError::msg("Sender account not found"))?;
        tx.set_nonce(account.nonce);
        let tx = R::Transaction::from(tx.clone());

        // TODO: Consensus handle system txs
        // https://github.com/bnb-chain/reth/blob/main/crates/bsc/evm/src/execute.rs#L602

        let recovered = Recovered::new_unchecked(tx.clone(), sender);

        let result_and_state = self.evm.transact(recovered).map_err(BlockExecutionError::other)?;
        let ResultAndState { result, state } = result_and_state;

        let gas_used = result.gas_used();
        self.gas_used += gas_used;
        self.receipts.push(self.receipt_builder.build_receipt(ReceiptBuilderCtx {
            tx: &tx,
            evm: &self.evm,
            result,
            state: &state,
            cumulative_gas_used: self.gas_used,
        }));
        self.evm.db_mut().commit(state);

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

impl<'a, DB, E, Spec, R> BlockExecutor for BscBlockExecutor<'a, E, Spec, R>
where
    DB: Database + 'a,
    E: Evm<DB = &'a mut State<DB>, Tx: FromRecoveredTx<R::Transaction>>,
    Spec: EthereumHardforks + BscHardforks + EthChainSpec + Hardforks,
    R: ReceiptBuilder<Transaction: SignedTransaction, Receipt: TxReceipt<Log = Log>>,
    <R as ReceiptBuilder>::Transaction: Unpin + From<TransactionSigned>,
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

        // If first block deploy genesis contracts
        if self.evm.block().number == 1 {
            self.deploy_genesis_contracts(self.evm.block().beneficiary)?;
        }

        self.apply_upgrade_contracts_if_before_feynman()?;
        self.deploy_feynman_contracts(self.evm.block().beneficiary)?;

        // increment balances
        let balance_increments = post_block_balance_increments(
            &self.spec,
            self.evm.block(),
            self.ctx.ommers,
            self.ctx.withdrawals.as_deref(),
        );
        self.evm
            .db_mut()
            .increment_balances(balance_increments.clone())
            .map_err(|_| BlockValidationError::IncrementBalanceFailed)?;

        // TODO:
        // Consensus: Slash validator if not in turn
        // Consensus: Distribute rewards
        // Consensus: Update validator set

        Ok((
            self.evm,
            BlockExecutionResult {
                receipts: self.receipts,
                requests: Requests::default(),
                gas_used: self.gas_used,
            },
        ))
    }

    fn set_state_hook(&mut self, _hook: Option<Box<dyn OnStateHook>>) {}

    fn evm_mut(&mut self) -> &mut Self::Evm {
        &mut self.evm
    }
}
