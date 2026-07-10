//! EVM-backed Ethereum executor.

use crate::{
    execution::{
        block_requests_from_receipts, execute_transaction_with_commit_condition,
        post_block_balance_state_changes, post_execution_system_call_state_changes,
        pre_execution_system_call_state_changes, transaction_blob_gas_used, BlockExecutionContext,
        BlockSystemCalls,
    },
    EthBlockExecutionCtx, EthTxEnv, RethReceiptBuilder,
};
use alloc::{borrow::Cow, boxed::Box, vec::Vec};
use alloy_consensus::{Header, TxType};
use alloy_eips::{eip2718::Typed2718, eip4895::Withdrawal};
use alloy_primitives::{Address, B256};
use evm2::{evm::BlockStateAccumulator, interpreter::Host, BaseEvmTypes, Evm, TxResult};
use reth_ethereum_primitives::{EthPrimitives, Receipt};
use reth_evm::{
    BlockExecutionError, BlockExecutionOutput, BlockExecutor, CommitChanges, GasOutput,
    ReceiptBuilder, ReceiptBuilderCtx,
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
    blob_gas_used: u64,
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
}

impl<'a> BlockExecutor for EthBlockExecutor<'a> {
    type Primitives = EthPrimitives;
    type Evm = Evm<'a, BaseEvmTypes>;
    type Transaction = EthTxEnv;
    type TransactionResult = TxResult;
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
        let tx_hash = transaction.tx_hash();
        let transaction = transaction.into_envelope();
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
        .map_err(|err| BlockExecutionError::evm(err, tx_hash))?
        else {
            return Ok(None)
        };
        let tx_gas_used = outcome.tx_gas_used();
        let state_gas_used = outcome.state_gas_spent();
        self.cumulative_gas_used += tx_gas_used;
        self.blob_gas_used += tx_blob_gas_used;
        self.receipts.push(RethReceiptBuilder.build_receipt(ReceiptBuilderCtx {
            tx_type,
            result: outcome,
            cumulative_gas_used: self.cumulative_gas_used,
        }));
        Ok(Some(GasOutput::new(tx_gas_used, state_gas_used)))
    }

    fn receipts(&self) -> &[Receipt] {
        &self.receipts
    }

    fn finish(mut self) -> Result<BlockExecutionOutput<Receipt>, BlockExecutionError> {
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

        let mut output = RethReceiptBuilder.build_block_output(
            self.receipts,
            self.block_state,
            self.blob_gas_used,
        );
        output.result.requests = requests;

        Ok(output)
    }
}

fn emit_hashed_state(hook: &mut HashedStateUpdateHook, state: HashedPostState) {
    if let Some(hook) = hook.as_mut() {
        hook(state);
    }
}
