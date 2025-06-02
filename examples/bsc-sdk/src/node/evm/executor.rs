use super::patch::{patch_mainnet_after_tx, patch_mainnet_before_tx};
use crate::{
    consensus::{MAX_SYSTEM_REWARD, SYSTEM_ADDRESS, SYSTEM_REWARD_PERCENT},
    evm::transaction::BscTxEnv,
    hardforks::BscHardforks,
    system_contracts::{
        get_upgrade_system_contracts, is_system_transaction, SystemContract, SYSTEM_REWARD_CONTRACT,
    },
};
use alloy_consensus::{Transaction, TxReceipt};
use alloy_eips::{eip7685::Requests, Encodable2718};
use alloy_evm::{block::ExecutableTx, eth::receipt_builder::ReceiptBuilderCtx};
use alloy_primitives::{Address, TxKind, U256};
use alloy_sol_macro::sol;
use alloy_sol_types::SolCall;
use reth_chainspec::{EthChainSpec, EthereumHardforks, Hardforks};
use reth_evm::{
    block::{BlockValidationError, CommitChanges},
    eth::{receipt_builder::ReceiptBuilder, EthBlockExecutionCtx},
    execute::{BlockExecutionError, BlockExecutor},
    Database, Evm, FromRecoveredTx, FromTxWithEncoded, IntoTxEnv, OnStateHook, RecoveredTx,
};
use reth_primitives::TransactionSigned;
use reth_primitives_traits::SignerRecoverable;
use reth_provider::BlockExecutionResult;
use reth_revm::State;
use revm::{
    context::{
        result::{ExecutionResult, ResultAndState},
        TxEnv,
    },
    state::Bytecode,
    Database as _, DatabaseCommit,
};
use std::str::FromStr;

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
    _ctx: EthBlockExecutionCtx<'a>,
}

impl<'a, DB, EVM, Spec, R: ReceiptBuilder> BscBlockExecutor<'a, EVM, Spec, R>
where
    DB: Database + 'a,
    EVM: Evm<
        DB = &'a mut State<DB>,
        Tx: FromRecoveredTx<R::Transaction>
                + FromRecoveredTx<TransactionSigned>
                + FromTxWithEncoded<TransactionSigned>,
    >,
    Spec: EthereumHardforks + BscHardforks + EthChainSpec + Hardforks + Clone,
    R: ReceiptBuilder<Transaction = TransactionSigned, Receipt: TxReceipt>,
    <R as ReceiptBuilder>::Transaction: Unpin + From<TransactionSigned>,
    <EVM as alloy_evm::Evm>::Tx: FromTxWithEncoded<<R as ReceiptBuilder>::Transaction>,
    BscTxEnv<TxEnv>: IntoTxEnv<<EVM as alloy_evm::Evm>::Tx>,
    R::Transaction: Into<TransactionSigned>,
{
    /// Creates a new BscBlockExecutor.
    pub fn new(
        evm: EVM,
        _ctx: EthBlockExecutionCtx<'a>,
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
            _ctx,
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
        for tx in txs {
            self.transact_system_tx(&tx, beneficiary)?;
        }
        Ok(())
    }

    /// Initializes the genesis contracts
    fn deploy_genesis_contracts(
        &mut self,
        beneficiary: Address,
    ) -> Result<(), BlockExecutionError> {
        let txs = self.system_contracts.genesis_contracts_txs();

        for tx in txs {
            self.transact_system_tx(&tx, beneficiary)?;
        }
        Ok(())
    }

    pub(crate) fn transact_system_tx(
        &mut self,
        tx: &TransactionSigned,
        sender: Address,
    ) -> Result<(), BlockExecutionError> {
        // TODO: Consensus handle reverting slashing system txs (they shouldnt be in the block)
        // https://github.com/bnb-chain/reth/blob/main/crates/bsc/evm/src/execute.rs#L602

        let account = self
            .evm
            .db_mut()
            .basic(sender)
            .map_err(BlockExecutionError::other)?
            .unwrap_or_default();

        let tx_env = BscTxEnv {
            base: TxEnv {
                caller: sender,
                kind: TxKind::Call(tx.to().unwrap()),
                nonce: account.nonce,
                gas_limit: u64::MAX / 2,
                value: tx.value(),
                data: tx.input().clone(),
                // Setting the gas price to zero enforces that no value is transferred as part of
                // the call, and that the call will not count against the block's
                // gas limit
                gas_price: 0,
                // The chain ID check is not relevant here and is disabled if set to None
                chain_id: Some(56),
                // Setting the gas priority fee to None ensures the effective gas price is
                //derived         // from the `gas_price` field, which we need to be zero
                gas_priority_fee: None,
                access_list: Default::default(),
                // blob fields can be None for this tx
                blob_hashes: Vec::new(),
                max_fee_per_blob_gas: 0,
                tx_type: 0,
                authorization_list: Default::default(),
            },
            is_system_transaction: true,
        };

        let result_and_state = self.evm.transact(tx_env).map_err(BlockExecutionError::other)?;

        let ResultAndState { result, state } = result_and_state;

        let tx = tx.clone();
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

    /// Handle slash system tx
    fn handle_slash_tx(&mut self, tx: &TransactionSigned) -> Result<(), BlockExecutionError> {
        sol!(
            function slash(
                address amounts,
            );
        );

        let input = tx.input();
        let is_slash_tx = input.len() >= 4 && input[..4] == slashCall::SELECTOR;

        if is_slash_tx {
            let signer = tx.recover_signer().map_err(BlockExecutionError::other)?;
            self.transact_system_tx(tx, signer)?;
        }

        Ok(())
    }

    /// Distributes block rewards to the validator.
    fn distribute_block_rewards(&mut self, validator: Address) -> Result<(), BlockExecutionError> {
        let system_account = self
            .evm
            .db_mut()
            .load_cache_account(SYSTEM_ADDRESS)
            .map_err(BlockExecutionError::other)?;

        if system_account.account.is_none() ||
            system_account.account.as_ref().unwrap().info.balance == U256::ZERO
        {
            return Ok(());
        }

        let (mut block_reward, mut transition) = system_account.drain_balance();
        transition.info = None;
        self.evm.db_mut().apply_transition(vec![(SYSTEM_ADDRESS, transition)]);
        let balance_increment = vec![(validator, block_reward)];

        self.evm
            .db_mut()
            .increment_balances(balance_increment)
            .map_err(BlockExecutionError::other)?;

        let system_reward_balance = self
            .evm
            .db_mut()
            .basic(Address::from_str(SYSTEM_REWARD_CONTRACT).unwrap())
            .map_err(BlockExecutionError::other)?
            .unwrap_or_default()
            .balance;

        // Kepler introduced a max system reward limit, so we need to pay the system reward to the
        // system contract if the limit is not exceeded.
        if !self.spec.is_kepler_active_at_timestamp(self.evm.block().timestamp) &&
            system_reward_balance < U256::from(MAX_SYSTEM_REWARD)
        {
            let reward_to_system = block_reward >> SYSTEM_REWARD_PERCENT;
            if reward_to_system > 0 {
                let tx = self.system_contracts.pay_system_tx(reward_to_system);
                self.transact_system_tx(&tx, validator)?;
            }

            block_reward -= reward_to_system;
        }

        let tx = self.system_contracts.pay_validator_tx(validator, block_reward);
        self.transact_system_tx(&tx, validator)?;
        Ok(())
    }
}

impl<'a, DB, E, Spec, R> BlockExecutor for BscBlockExecutor<'a, E, Spec, R>
where
    DB: Database + 'a,
    E: Evm<
        DB = &'a mut State<DB>,
        Tx: FromRecoveredTx<R::Transaction>
                + FromRecoveredTx<TransactionSigned>
                + FromTxWithEncoded<TransactionSigned>,
    >,
    Spec: EthereumHardforks + BscHardforks + EthChainSpec + Hardforks,
    R: ReceiptBuilder<Transaction = TransactionSigned, Receipt: TxReceipt>,
    <R as ReceiptBuilder>::Transaction: Unpin + From<TransactionSigned>,
    <E as alloy_evm::Evm>::Tx: FromTxWithEncoded<<R as ReceiptBuilder>::Transaction>,
    BscTxEnv<TxEnv>: IntoTxEnv<<E as alloy_evm::Evm>::Tx>,
    R::Transaction: Into<TransactionSigned>,
{
    type Transaction = TransactionSigned;
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

    fn execute_transaction_with_commit_condition(
        &mut self,
        _tx: impl ExecutableTx<Self>,
        _f: impl FnOnce(&ExecutionResult<<Self::Evm as Evm>::HaltReason>) -> CommitChanges,
    ) -> Result<Option<u64>, BlockExecutionError> {
        Ok(Some(0))
    }

    fn execute_transaction_with_result_closure(
        &mut self,
        tx: impl ExecutableTx<Self>
            + IntoTxEnv<<E as alloy_evm::Evm>::Tx>
            + RecoveredTx<TransactionSigned>,
        f: impl for<'b> FnOnce(&'b ExecutionResult<<E as alloy_evm::Evm>::HaltReason>),
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
        // Consensus: Verify turn length

        // If first block deploy genesis contracts
        if self.evm.block().number == 1 {
            self.deploy_genesis_contracts(self.evm.block().beneficiary)?;
        }

        self.apply_upgrade_contracts_if_before_feynman()?;

        if self.spec.is_feynman_active_at_timestamp(self.evm.block().timestamp) {
            self.deploy_feynman_contracts(self.evm.block().beneficiary)?;
        }

        self.distribute_block_rewards(self.evm.block().beneficiary)?;

        let system_txs = self.system_txs.clone();
        for tx in system_txs {
            self.handle_slash_tx(&tx)?;
        }

        // TODO:
        // Consensus: Slash validator if not in turn
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

    fn evm(&self) -> &Self::Evm {
        &self.evm
    }
}
