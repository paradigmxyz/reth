//! EVM-backed Ethereum executor.

use crate::{
    execution::{
        block_requests_from_receipts, commit_detached_transaction,
        execute_transaction_with_commit_condition, execute_transaction_without_commit,
        post_block_balance_state_changes, post_execution_system_call_state_changes,
        pre_execution_system_call_state_changes, transaction_blob_gas_used, BlockExecutionContext,
        BlockSystemCalls, EthExecutionError,
    },
    EthBlockExecutionCtx, EthTxEnv, RethReceiptBuilder,
};
use alloc::{borrow::Cow, boxed::Box, vec::Vec};
use alloy_consensus::{Header, TxType};
use alloy_eip7928::{BlockAccessIndex, BlockAccessList};
use alloy_eips::{eip2718::Typed2718, eip4895::Withdrawal};
use alloy_primitives::{Address, B256};
use evm2::{
    evm::{Bal, BlockStateAccumulator},
    interpreter::Host,
    BaseEvmTypes, Evm, TxResult, TxResultWithState,
};
use reth_ethereum_primitives::{EthPrimitives, Receipt};
use reth_evm::{
    BlockExecutionError, BlockExecutionOutput, BlockExecutor, BlockValidationError, CommitChanges,
    GasOutput, ReceiptBuilder, ReceiptBuilderCtx,
};
use reth_trie_common::HashedPostState;

/// Configured Ethereum block executor backed by evm2.
#[expect(missing_debug_implementations)]
pub struct EthBlockExecutor<'a> {
    evm: Evm<'a, BaseEvmTypes>,
    spec_id: evm2::SpecId,
    block_number: u64,
    block_beneficiary: Address,
    parent_hash: B256,
    parent_beacon_block_root: Option<B256>,
    ommers: &'a [Header],
    withdrawals: Option<Cow<'a, [Withdrawal]>>,
    chain_id: u64,
    deposit_contract_address: Option<Address>,
    block_state: BlockStateAccumulator,
    hashed_state_mode: HashedStateMode,
    hashed_state_update_hook: HashedStateUpdateHook,
    receipts: Vec<Receipt>,
    cumulative_gas_used: u64,
    block_regular_gas_used: u64,
    block_state_gas_used: u64,
    block_gas_limit: u64,
    tx_gas_limit_cap: u64,
    separate_block_gas: bool,
    blob_gas_used: u64,
}

/// Detached Ethereum transaction result with the metadata needed for canonical receipt commit.
#[derive(Debug)]
pub struct EthTransactionResultWithState {
    result: TxResultWithState,
    tx_type: TxType,
    blob_gas_used: u64,
    tx_gas_limit: u64,
}

type HashedStateUpdateHook = Option<Box<dyn FnMut(HashedPostState) + Send>>;

/// Controls whether Ethereum execution streams trie-ready hashed post-state updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HashedStateMode {
    /// Do not stream hashed state updates.
    OutputOnly,
    /// Stream hashed state updates to the provided hook.
    StreamOnly,
}

impl HashedStateMode {
    /// Returns true if execution should stream hashed state updates.
    pub(crate) const fn stream(self) -> bool {
        matches!(self, Self::StreamOnly)
    }
}

impl<'a> EthBlockExecutor<'a> {
    /// Creates a configured Ethereum block executor.
    pub(crate) fn new(
        mut evm: Evm<'a, BaseEvmTypes>,
        context: EthBlockExecutionCtx<'a>,
        chain_id: u64,
        deposit_contract_address: Option<alloy_primitives::Address>,
        hashed_state_mode: HashedStateMode,
    ) -> Self {
        let spec_id = evm.spec_id();
        let block = *evm.block_env();
        let block_number = block.number.to::<u64>();
        let block_beneficiary = block.beneficiary;
        let block_gas_limit = block.gas_limit.to::<u64>();
        let tx_gas_limit_cap = evm.version().tx_gas_limit_cap;
        let separate_block_gas = evm.version().feature(evm2::EvmFeatures::EIP8037);

        Self {
            evm,
            spec_id,
            block_number,
            block_beneficiary,
            parent_hash: context.parent_hash,
            parent_beacon_block_root: context.parent_beacon_block_root,
            ommers: context.ommers,
            withdrawals: context.withdrawals,
            chain_id,
            deposit_contract_address,
            block_state: BlockStateAccumulator::new(),
            hashed_state_mode,
            hashed_state_update_hook: None,
            receipts: Vec::new(),
            cumulative_gas_used: 0,
            block_regular_gas_used: 0,
            block_state_gas_used: 0,
            block_gas_limit,
            tx_gas_limit_cap,
            separate_block_gas,
            blob_gas_used: 0,
        }
    }

    const fn block_context<'ctx>(
        chain_id: u64,
        deposit_contract_address: Option<Address>,
        parent_hash: B256,
        parent_beacon_block_root: Option<B256>,
        ommers: &'ctx [Header],
        withdrawals: Option<&'ctx [Withdrawal]>,
    ) -> BlockExecutionContext<'ctx> {
        BlockExecutionContext {
            chain_id,
            system_calls: Some(BlockSystemCalls { parent_hash, parent_beacon_block_root }),
            ommers: Some(ommers),
            withdrawals,
            deposit_contract_address,
        }
    }

    fn record_transaction(
        &mut self,
        tx_type: TxType,
        blob_gas_used: u64,
        outcome: TxResult,
    ) -> GasOutput {
        let tx_gas_used = outcome.tx_gas_used();
        let regular_gas_used = outcome.regular_gas_spent();
        let state_gas_used = outcome.state_gas_spent();
        self.block_regular_gas_used = self.block_regular_gas_used.saturating_add(regular_gas_used);
        self.block_state_gas_used = self.block_state_gas_used.saturating_add(state_gas_used);
        self.cumulative_gas_used += tx_gas_used;
        self.blob_gas_used += blob_gas_used;
        self.receipts.push(RethReceiptBuilder.build_receipt(ReceiptBuilderCtx {
            tx_type,
            result: outcome,
            cumulative_gas_used: self.cumulative_gas_used,
        }));
        GasOutput::new_with_regular(tx_gas_used, regular_gas_used, state_gas_used)
    }

    fn validate_transaction_gas_limit(
        &self,
        transaction_gas_limit: u64,
    ) -> Result<(), BlockExecutionError> {
        let unavailable = if self.separate_block_gas {
            let regular_available =
                self.block_gas_limit.saturating_sub(self.block_regular_gas_used);
            let state_available = self.block_gas_limit.saturating_sub(self.block_state_gas_used);
            let regular_limit = transaction_gas_limit.min(self.tx_gas_limit_cap);
            if regular_limit > regular_available {
                Some((regular_limit, regular_available))
            } else if transaction_gas_limit > state_available {
                Some((transaction_gas_limit, state_available))
            } else {
                None
            }
        } else {
            let available = self.block_gas_limit.saturating_sub(self.cumulative_gas_used);
            (transaction_gas_limit > available).then_some((transaction_gas_limit, available))
        };

        if let Some((transaction_gas_limit, block_available_gas)) = unavailable {
            return Err(BlockValidationError::TransactionGasLimitMoreThanAvailableBlockGas {
                transaction_gas_limit,
                block_available_gas,
            }
            .into())
        }
        Ok(())
    }

    const fn block_access_list_builder_enabled(&self) -> bool {
        self.evm.state().bal_builder().is_some()
    }
}

impl<'a> BlockExecutor for EthBlockExecutor<'a> {
    type Primitives = EthPrimitives;
    type Evm = Evm<'a, BaseEvmTypes>;
    type Transaction = EthTxEnv;
    type TransactionResult = TxResult;
    type TransactionResultWithState = EthTransactionResultWithState;
    type BlockAccessList = Bal;
    type TransactionOutput = GasOutput;

    fn evm(&self) -> &Self::Evm {
        &self.evm
    }

    fn evm_mut(&mut self) -> &mut Self::Evm {
        &mut self.evm
    }

    fn set_state_hook(&mut self, hook: impl FnMut(HashedPostState) + Send + 'static) -> bool {
        if self.hashed_state_mode == HashedStateMode::OutputOnly {
            self.hashed_state_mode = HashedStateMode::StreamOnly;
        }

        self.hashed_state_update_hook = Some(Box::new(hook));
        true
    }

    fn convert_block_access_list(
        block_access_list: &BlockAccessList,
    ) -> Result<Self::BlockAccessList, BlockExecutionError> {
        Bal::try_from(block_access_list.as_slice()).map_err(BlockExecutionError::other)
    }

    fn set_block_access_list(
        &mut self,
        block_access_list: alloc::sync::Arc<Self::BlockAccessList>,
    ) {
        self.evm.state_mut().set_bal(block_access_list);
    }

    fn set_block_access_index(&mut self, index: BlockAccessIndex) {
        self.evm.state_mut().set_bal_index(index);
    }

    fn enable_block_access_list_builder(&mut self) {
        self.evm.state_mut().enable_bal_builder();
        self.evm.state_mut().reset_bal_index();
    }

    fn take_block_access_list(&mut self) -> Option<BlockAccessList> {
        self.evm.state_mut().take_bal_builder().map(Into::into)
    }

    fn apply_pre_execution_changes(&mut self) -> Result<(), BlockExecutionError> {
        let context = Self::block_context(
            self.chain_id,
            self.deposit_contract_address,
            self.parent_hash,
            self.parent_beacon_block_root,
            self.ommers,
            None,
        );
        pre_execution_system_call_state_changes(
            &mut self.evm,
            &mut self.block_state,
            self.hashed_state_mode.stream(),
            &mut |state| emit_hashed_state(&mut self.hashed_state_update_hook, state),
            self.spec_id,
            self.block_number,
            context,
        )
        .map_err(Into::into)
    }

    fn execute_transaction_with_commit_condition(
        &mut self,
        transaction: Self::Transaction,
        f: impl FnOnce(&Self::TransactionResult) -> CommitChanges,
    ) -> Result<Option<Self::TransactionOutput>, BlockExecutionError> {
        if self.block_access_list_builder_enabled() {
            self.set_block_access_index(BlockAccessIndex::new(self.receipts.len() as u64 + 1));
        }
        let tx_hash = transaction.tx_hash();
        let transaction = transaction.into_envelope();
        self.validate_transaction_gas_limit(transaction.gas_limit())?;
        let tx_blob_gas_used = transaction_blob_gas_used(&transaction);
        let tx_type =
            TxType::try_from(transaction.ty()).expect("transaction envelope has valid type");
        let Some(outcome) = execute_transaction_with_commit_condition(
            &mut self.evm,
            &mut self.block_state,
            self.hashed_state_mode.stream(),
            &mut |state| emit_hashed_state(&mut self.hashed_state_update_hook, state),
            &transaction,
            f,
        )
        .map_err(|err| map_transaction_execution_error(err, tx_hash))?
        else {
            return Ok(None)
        };
        Ok(Some(self.record_transaction(tx_type, tx_blob_gas_used, outcome)))
    }

    fn execute_transaction_without_commit(
        &mut self,
        transaction: Self::Transaction,
    ) -> Result<Self::TransactionResultWithState, BlockExecutionError> {
        let tx_hash = transaction.tx_hash();
        let transaction = transaction.into_envelope();
        let tx_gas_limit = transaction.gas_limit();
        let blob_gas_used = transaction_blob_gas_used(&transaction);
        let tx_type =
            TxType::try_from(transaction.ty()).expect("transaction envelope has valid type");
        let result = execute_transaction_without_commit(&mut self.evm, &transaction)
            .map_err(|err| map_transaction_execution_error(err, tx_hash))?;
        Ok(EthTransactionResultWithState { result, tx_type, blob_gas_used, tx_gas_limit })
    }

    fn commit_transaction(
        &mut self,
        output: Self::TransactionResultWithState,
    ) -> Result<Self::TransactionOutput, BlockExecutionError> {
        self.validate_transaction_gas_limit(output.tx_gas_limit)?;
        if self.block_access_list_builder_enabled() {
            self.set_block_access_index(BlockAccessIndex::new(self.receipts.len() as u64 + 1));
        }
        let outcome = commit_detached_transaction(
            &mut self.evm,
            &mut self.block_state,
            self.hashed_state_mode.stream(),
            &mut |state| emit_hashed_state(&mut self.hashed_state_update_hook, state),
            output.result,
        );
        Ok(self.record_transaction(output.tx_type, output.blob_gas_used, outcome))
    }

    fn receipts(&self) -> &[Receipt] {
        &self.receipts
    }

    fn finish_with_block_access_list(
        mut self,
    ) -> Result<(BlockExecutionOutput<Receipt>, Option<BlockAccessList>), BlockExecutionError> {
        if self.block_access_list_builder_enabled() {
            self.set_block_access_index(BlockAccessIndex::new(self.receipts.len() as u64 + 1));
        }
        let context = Self::block_context(
            self.chain_id,
            self.deposit_contract_address,
            self.parent_hash,
            self.parent_beacon_block_root,
            self.ommers,
            None,
        );
        let mut requests = block_requests_from_receipts(self.spec_id, context, &self.receipts)?;
        post_execution_system_call_state_changes(
            &mut self.evm,
            &mut self.block_state,
            self.hashed_state_mode.stream(),
            &mut |state| emit_hashed_state(&mut self.hashed_state_update_hook, state),
            self.spec_id,
            context,
            &mut requests,
        )
        .map_err(BlockExecutionError::from)?;

        let withdrawals = self.withdrawals.clone();
        let context = Self::block_context(
            self.chain_id,
            self.deposit_contract_address,
            self.parent_hash,
            self.parent_beacon_block_root,
            self.ommers,
            withdrawals.as_deref(),
        );
        post_block_balance_state_changes(
            &mut self.evm,
            &mut self.block_state,
            self.hashed_state_mode.stream(),
            &mut |state| emit_hashed_state(&mut self.hashed_state_update_hook, state),
            self.spec_id,
            self.block_number,
            self.block_beneficiary,
            context.ommers,
            context.withdrawals,
        )
        .map_err(BlockExecutionError::from)?;

        let block_access_list = self.take_block_access_list();
        let block_gas_used = final_block_gas_used(
            self.separate_block_gas,
            self.cumulative_gas_used,
            self.block_regular_gas_used,
            self.block_state_gas_used,
        );
        let mut output = RethReceiptBuilder.build_block_output(
            self.receipts,
            self.block_state,
            self.blob_gas_used,
        );
        output.result.gas_used = block_gas_used;
        output.result.requests = requests;

        Ok((output, block_access_list))
    }
}

const fn final_block_gas_used(
    separate_block_gas: bool,
    cumulative_gas_used: u64,
    block_regular_gas_used: u64,
    block_state_gas_used: u64,
) -> u64 {
    if separate_block_gas {
        if block_regular_gas_used > block_state_gas_used {
            block_regular_gas_used
        } else {
            block_state_gas_used
        }
    } else {
        cumulative_gas_used
    }
}

fn map_transaction_execution_error(err: EthExecutionError, tx_hash: B256) -> BlockExecutionError {
    match err {
        EthExecutionError::BlockAccessListNotCovered => {
            BlockValidationError::BlockAccessListNotCovered.into()
        }
        err => BlockExecutionError::evm(err, tx_hash),
    }
}

fn emit_hashed_state(hook: &mut HashedStateUpdateHook, state: HashedPostState) {
    if let Some(hook) = hook.as_mut() {
        hook(state);
    }
}

#[cfg(test)]
mod tests {
    use super::{final_block_gas_used, map_transaction_execution_error};
    use crate::execution::EthExecutionError;
    use alloy_primitives::B256;
    use reth_evm::{BlockExecutionError, BlockValidationError};

    #[test]
    fn amsterdam_header_uses_bottleneck_gas_dimension() {
        assert_eq!(final_block_gas_used(true, 90, 70, 120), 120);
        assert_eq!(final_block_gas_used(true, 90, 130, 120), 130);
        assert_eq!(final_block_gas_used(false, 90, 130, 120), 90);
    }

    #[test]
    fn bal_not_covered_remains_a_validation_error() {
        assert!(matches!(
            map_transaction_execution_error(
                EthExecutionError::BlockAccessListNotCovered,
                B256::ZERO
            ),
            BlockExecutionError::Validation(BlockValidationError::BlockAccessListNotCovered)
        ));
    }
}
