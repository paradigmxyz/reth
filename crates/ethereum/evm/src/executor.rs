//! EVM-backed Ethereum executor.

use crate::{
    execution::{
        base_block_reward, block_requests_from_receipts, commit_detached_transaction,
        execute_transaction_without_commit, post_block_balance_state_changes,
        post_execution_system_call_state_changes, pre_execution_system_call_state_changes,
        BlockExecutionContext, BlockSystemCalls, EthExecutionError,
    },
    factory::{EthBlockExecutorFactory, EvmFactory},
    EthBlockExecutionCtx, EthEvmEnv, RethReceiptBuilder,
};
use alloc::{boxed::Box, sync::Arc, vec::Vec};
use alloy_consensus::{
    transaction::{TransactionEnvelope, TxHashRef},
    Header, Transaction, TxType as EthTxType,
};
use alloy_eip7928::{BlockAccessIndex, BlockAccessList};
use alloy_eips::{eip2718::Typed2718, eip4895::Withdrawal, eip7685::Requests};
use alloy_primitives::{Address, B256};
use evm2::{
    ethereum::TxEnvelope,
    evm::{Bal, BlockStateAccumulator, StateChangeSource},
    interpreter::Host,
    BaseEvmTypes, Evm, EvmTypes, TxResultWithState,
};
use reth_chainspec::EthChainSpec;
use reth_ethereum_forks::EthereumHardforks;
use reth_ethereum_primitives::{Receipt, TransactionSigned};
use reth_evm::{
    BlockExecutionError, BlockExecutionOutput, BlockExecutor, BlockTransactionResult,
    BlockValidationError, ExecutorTx, GasOutput, ReceiptBuilder, ReceiptBuilderCtx, RecoveredTx,
};
use reth_execution_types::BlockExecutionResult;
use reth_trie_common::HashedPostState;

/// Configured Ethereum block executor backed by evm2.
#[expect(missing_debug_implementations)]
pub struct EthBlockExecutor<'a, T = BaseEvmTypes, R = RethReceiptBuilder>
where
    T: EvmTypes<Tx: Typed2718, TxResultExt: Send>,
    R: ReceiptBuilder,
{
    evm: Evm<'a, T>,
    ctx: EthBlockExecutionCtx<'a>,
    receipt_builder: R,
    spec_id: evm2::SpecId,
    base_block_reward: Option<u128>,
    deposit_contract_address: Option<Address>,
    block_state: BlockStateAccumulator,
    hashed_state_update_hook: HashedStateUpdateHook,
    receipts: Vec<R::Receipt>,
    cumulative_gas_used: u64,
    block_regular_gas_used: u64,
    block_state_gas_used: u64,
    separate_block_gas: bool,
    blob_gas_used: u64,
    bal_index_offset: u64,
}

/// Detached Ethereum transaction result with the metadata needed for canonical receipt commit.
#[derive(Debug)]
pub struct EthTransactionResultWithState<T = BaseEvmTypes, TxType = EthTxType>
where
    T: EvmTypes,
{
    result: TxResultWithState<T>,
    tx_type: TxType,
    blob_gas_used: u64,
}

impl<T: EvmTypes, TxType> BlockTransactionResult<T> for EthTransactionResultWithState<T, TxType> {
    fn result(&self) -> &TxResultWithState<T> {
        &self.result
    }
}

type HashedStateUpdateHook = Option<Box<dyn FnMut(HashedPostState) + Send>>;

impl<'a, T, R> EthBlockExecutor<'a, T, R>
where
    T: EvmTypes<Tx: Typed2718, TxResultExt: Send>,
    R: ReceiptBuilder,
{
    /// Creates a configured Ethereum block executor.
    pub fn new<C>(
        mut evm: Evm<'a, T>,
        ctx: EthBlockExecutionCtx<'a>,
        chain_spec: &C,
        receipt_builder: R,
    ) -> Self
    where
        C: EthChainSpec + EthereumHardforks + ?Sized,
    {
        // The executor stores evm2's base spec ID, while custom type families may expose a
        // richer host spec ID that converts into it.
        #[allow(clippy::useless_conversion)]
        let spec_id = evm.spec_id().into();
        let block_number = evm.block_env().number.to::<u64>();
        let separate_block_gas = evm.version().feature(evm2::EvmFeatures::EIP8037);

        Self {
            evm,
            ctx,
            receipt_builder,
            spec_id,
            base_block_reward: base_block_reward(chain_spec, block_number),
            deposit_contract_address: chain_spec
                .deposit_contract()
                .map(|contract| contract.address),
            block_state: BlockStateAccumulator::new(),
            hashed_state_update_hook: None,
            receipts: Vec::new(),
            cumulative_gas_used: 0,
            block_regular_gas_used: 0,
            block_state_gas_used: 0,
            separate_block_gas,
            blob_gas_used: 0,
            bal_index_offset: 0,
        }
    }

    const fn block_context<'ctx>(
        deposit_contract_address: Option<Address>,
        parent_hash: B256,
        parent_beacon_block_root: Option<B256>,
        ommers: &'ctx [Header],
        withdrawals: Option<&'ctx [Withdrawal]>,
    ) -> BlockExecutionContext<'ctx> {
        BlockExecutionContext {
            system_calls: Some(BlockSystemCalls { parent_hash, parent_beacon_block_root }),
            ommers: Some(ommers),
            withdrawals,
            deposit_contract_address,
        }
    }

    const fn block_access_list_builder_enabled(&self) -> bool {
        self.evm.state().bal_builder().is_some()
    }

    #[inline]
    const fn set_transaction_block_access_index(&mut self) {
        if self.block_access_list_builder_enabled() {
            let index = self.receipts.len() as u64 + 1 + self.bal_index_offset;
            self.evm.state_mut().set_bal_index(BlockAccessIndex::new(index));
        }
    }
}

impl<'a, T, R> BlockExecutor for EthBlockExecutor<'a, T, R>
where
    T: EvmTypes<Tx: Typed2718, TxResultExt: Send>,
    R: ReceiptBuilder,
{
    type Transaction = R::Transaction;
    type Receipt = R::Receipt;
    type Evm = Evm<'a, T>;
    type TransactionResultWithState =
        EthTransactionResultWithState<T, <R::Transaction as TransactionEnvelope>::TxType>;
    type BlockAccessList = Bal;

    fn evm(&self) -> &Self::Evm {
        &self.evm
    }

    fn evm_mut(&mut self) -> &mut Self::Evm {
        &mut self.evm
    }

    fn set_state_hook(&mut self, hook: impl FnMut(HashedPostState) + Send + 'static) -> bool {
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
            self.deposit_contract_address,
            self.ctx.parent_hash,
            self.ctx.parent_beacon_block_root,
            self.ctx.ommers,
            None,
        );
        let block_number = self.evm.block_env().number.to::<u64>();
        let stream_hashed_state = self.hashed_state_update_hook.is_some();
        pre_execution_system_call_state_changes(
            &mut self.evm,
            &mut self.block_state,
            stream_hashed_state,
            &mut |state| emit_hashed_state(&mut self.hashed_state_update_hook, state),
            self.spec_id,
            block_number,
            context,
        )
        .map_err(Into::into)
    }

    fn execute_transaction_without_commit(
        &mut self,
        transaction: impl ExecutorTx<Self>,
    ) -> Result<Self::TransactionResultWithState, BlockExecutionError> {
        let (transaction, tx) = transaction.into_parts();
        let tx_hash = *tx.tx().tx_hash();
        self.set_transaction_block_access_index();
        let transaction_gas_limit = tx.tx().gas_limit();
        let block_gas_limit = self.evm.block_env().gas_limit.to::<u64>();
        let unavailable = if self.separate_block_gas {
            let regular_available = block_gas_limit.saturating_sub(self.block_regular_gas_used);
            let state_available = block_gas_limit.saturating_sub(self.block_state_gas_used);
            let regular_limit = transaction_gas_limit.min(self.evm.version().tx_gas_limit_cap);
            if regular_limit > regular_available {
                Some((regular_limit, regular_available))
            } else if transaction_gas_limit > state_available {
                Some((transaction_gas_limit, state_available))
            } else {
                None
            }
        } else {
            let available = block_gas_limit.saturating_sub(self.cumulative_gas_used);
            (transaction_gas_limit > available).then_some((transaction_gas_limit, available))
        };
        if let Some((transaction_gas_limit, block_available_gas)) = unavailable {
            return Err(BlockValidationError::TransactionGasLimitMoreThanAvailableBlockGas {
                transaction_gas_limit,
                block_available_gas,
            }
            .into())
        }
        let blob_gas_used = tx.tx().blob_gas_used().unwrap_or_default();
        let tx_type = tx.tx().tx_type();
        let result = execute_transaction_without_commit(&mut self.evm, &transaction)
            .map_err(|err| map_transaction_execution_error(err, tx_hash))?;
        Ok(EthTransactionResultWithState { result, tx_type, blob_gas_used })
    }

    fn commit_transaction(
        &mut self,
        output: Self::TransactionResultWithState,
    ) -> Result<GasOutput, BlockExecutionError> {
        let EthTransactionResultWithState { result, tx_type, blob_gas_used } = output;
        let stream_hashed_state = self.hashed_state_update_hook.is_some();
        let outcome = commit_detached_transaction(
            &mut self.evm,
            &mut self.block_state,
            stream_hashed_state,
            &mut |state| emit_hashed_state(&mut self.hashed_state_update_hook, state),
            result,
        );
        let tx_gas_used = outcome.tx_gas_used();
        let regular_gas_used = outcome.regular_gas_spent();
        let state_gas_used = outcome.state_gas_spent();
        self.block_regular_gas_used = self.block_regular_gas_used.saturating_add(regular_gas_used);
        self.block_state_gas_used = self.block_state_gas_used.saturating_add(state_gas_used);
        self.cumulative_gas_used += tx_gas_used;
        self.blob_gas_used += blob_gas_used;
        self.receipts.push(self.receipt_builder.build_receipt(ReceiptBuilderCtx {
            tx_type,
            result: outcome,
            cumulative_gas_used: self.cumulative_gas_used,
        }));
        Ok(GasOutput::new_with_regular(tx_gas_used, regular_gas_used, state_gas_used))
    }

    fn receipts(&self) -> &[Self::Receipt] {
        &self.receipts
    }

    fn finish_with_block_access_list(
        mut self,
    ) -> Result<(BlockExecutionOutput<Self::Receipt>, Option<BlockAccessList>), BlockExecutionError>
    {
        self.set_transaction_block_access_index();
        let context = Self::block_context(
            self.deposit_contract_address,
            self.ctx.parent_hash,
            self.ctx.parent_beacon_block_root,
            self.ctx.ommers,
            None,
        );
        let mut requests = block_requests_from_receipts(self.spec_id, context, &self.receipts)?;
        let stream_hashed_state = self.hashed_state_update_hook.is_some();
        post_execution_system_call_state_changes(
            &mut self.evm,
            &mut self.block_state,
            stream_hashed_state,
            &mut |state| emit_hashed_state(&mut self.hashed_state_update_hook, state),
            self.spec_id,
            context,
            &mut requests,
        )
        .map_err(BlockExecutionError::from)?;

        let withdrawals = self.ctx.withdrawals.clone();
        let context = Self::block_context(
            self.deposit_contract_address,
            self.ctx.parent_hash,
            self.ctx.parent_beacon_block_root,
            self.ctx.ommers,
            withdrawals.as_deref(),
        );
        let block_number = self.evm.block_env().number.to::<u64>();
        let block_beneficiary = self.evm.block_env().beneficiary;
        post_block_balance_state_changes(
            &mut self.evm,
            &mut self.block_state,
            stream_hashed_state,
            &mut |state| emit_hashed_state(&mut self.hashed_state_update_hook, state),
            self.base_block_reward,
            block_number,
            block_beneficiary,
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
        let output = BlockExecutionOutput::new(
            BlockExecutionResult {
                receipts: self.receipts,
                requests,
                gas_used: block_gas_used,
                blob_gas_used: self.blob_gas_used,
            },
            self.block_state,
        );

        Ok((output, block_access_list))
    }
}

/// One block execution segment inside a merged big-block payload.
pub struct EthBigBlockSegment<'a, T = BaseEvmTypes>
where
    T: EvmTypes<Tx = TxEnvelope>,
{
    /// Transaction index at which this segment starts.
    pub start_tx: usize,
    /// EVM environment for this segment.
    pub evm_env: EthEvmEnv<T>,
    /// Ethereum execution context for this segment.
    pub ctx: EthBlockExecutionCtx<'a>,
}

impl<'a, T> Clone for EthBigBlockSegment<'a, T>
where
    T: EvmTypes<Tx = TxEnvelope>,
{
    fn clone(&self) -> Self {
        Self { start_tx: self.start_tx, evm_env: self.evm_env.clone(), ctx: self.ctx.clone() }
    }
}

impl<'a, T> core::fmt::Debug for EthBigBlockSegment<'a, T>
where
    T: EvmTypes<Tx = TxEnvelope>,
    T::SpecId: core::fmt::Debug,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("EthBigBlockSegment")
            .field("start_tx", &self.start_tx)
            .field("evm_env", &self.evm_env)
            .field("ctx", &self.ctx)
            .finish()
    }
}

/// Execution plan for a merged big-block payload.
pub struct EthBigBlockPlan<'a, T = BaseEvmTypes>
where
    T: EvmTypes<Tx = TxEnvelope>,
{
    /// Ordered execution segments.
    pub segments: Vec<EthBigBlockSegment<'a, T>>,
    /// Total number of transactions across all segments.
    pub transaction_count: usize,
    /// Block hashes that must be available to `BLOCKHASH` during execution.
    pub block_hashes: Vec<(u64, B256)>,
}

impl<'a, T> Clone for EthBigBlockPlan<'a, T>
where
    T: EvmTypes<Tx = TxEnvelope>,
{
    fn clone(&self) -> Self {
        Self {
            segments: self.segments.clone(),
            transaction_count: self.transaction_count,
            block_hashes: self.block_hashes.clone(),
        }
    }
}

impl<'a, T> core::fmt::Debug for EthBigBlockPlan<'a, T>
where
    T: EvmTypes<Tx = TxEnvelope>,
    T::SpecId: core::fmt::Debug,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("EthBigBlockPlan")
            .field("segments", &self.segments)
            .field("transaction_count", &self.transaction_count)
            .field("block_hashes", &self.block_hashes)
            .finish()
    }
}

impl<'a, T> EthBigBlockPlan<'a, T>
where
    T: EvmTypes<Tx = TxEnvelope>,
{
    /// Creates a plan and adds hashes for the boundaries between segments.
    pub fn new(
        segments: Vec<EthBigBlockSegment<'a, T>>,
        prior_block_hashes: Vec<(u64, B256)>,
        transaction_count: usize,
    ) -> Self {
        let mut block_hashes = segments
            .iter()
            .skip(1)
            .map(|segment| {
                (
                    segment.evm_env.block.number.to::<u64>().saturating_sub(1),
                    segment.ctx.parent_hash,
                )
            })
            .collect::<Vec<_>>();
        block_hashes.extend(prior_block_hashes);
        block_hashes.sort_unstable_by_key(|(number, _)| *number);
        Self { segments, transaction_count, block_hashes }
    }

    fn hashes_for_block(&self, block_number: u64) -> impl Iterator<Item = (u64, B256)> + '_ {
        let min = block_number.saturating_sub(256);
        self.block_hashes
            .iter()
            .copied()
            .filter(move |(number, _)| *number >= min && *number < block_number)
    }

    fn segment_index_for_tx(&self, tx_index: usize) -> usize {
        self.segments.partition_point(|segment| segment.start_tx <= tx_index).saturating_sub(1)
    }
}

struct FinishedBigBlockSegment {
    state: BlockStateAccumulator,
    requests: Requests,
    gas_used: u64,
    blob_gas_used: u64,
}

/// Block executor for merged payloads that switches the EVM context at block boundaries.
#[expect(missing_debug_implementations)]
pub struct EthBigBlockExecutor<'a, C, F = crate::factory::RethEvmFactory>
where
    F: EvmFactory,
    F::Types: EvmTypes<Tx = TxEnvelope>,
    <F::Types as evm2::EvmTypesHost>::Tx: Typed2718,
    <F::Types as evm2::EvmTypesHost>::TxResultExt: Send,
{
    inner: EthBlockExecutor<'a, F::Types, &'a RethReceiptBuilder>,
    factory: &'a EthBlockExecutorFactory<RethReceiptBuilder, C, F>,
    chain_spec: Arc<C>,
    plan: EthBigBlockPlan<'a, F::Types>,
    next_segment: usize,
    tx_counter: usize,
    segment_receipt_start: usize,
    worker_segment: Option<usize>,
    initialized: bool,
    state: BlockStateAccumulator,
    requests: Requests,
    gas_used_offset: u64,
    blob_gas_used_offset: u64,
}

impl<'a, C, F> EthBigBlockExecutor<'a, C, F>
where
    C: EthereumHardforks,
    F: EvmFactory,
    F::Types: EvmTypes<Tx = TxEnvelope>,
    <F::Types as evm2::EvmTypesHost>::Tx: Typed2718,
    <F::Types as evm2::EvmTypesHost>::TxResultExt: Send,
{
    pub(crate) fn new(
        inner: EthBlockExecutor<'a, F::Types, &'a RethReceiptBuilder>,
        factory: &'a EthBlockExecutorFactory<RethReceiptBuilder, C, F>,
        chain_spec: Arc<C>,
        plan: EthBigBlockPlan<'a, F::Types>,
    ) -> Self {
        assert!(!plan.segments.is_empty(), "big-block execution requires at least one segment");
        Self {
            inner,
            factory,
            chain_spec,
            plan,
            next_segment: 1,
            tx_counter: 0,
            segment_receipt_start: 0,
            worker_segment: None,
            initialized: false,
            state: BlockStateAccumulator::new(),
            requests: Requests::default(),
            gas_used_offset: 0,
            blob_gas_used_offset: 0,
        }
    }

    fn configure_segment(&mut self, segment_idx: usize) {
        let segment = &self.plan.segments[segment_idx];
        let env = &segment.evm_env;
        if self.inner.evm.config_spec_id() == env.spec {
            self.inner.evm.set_block(env.block);
        } else {
            self.factory.reconfigure_evm(&mut self.inner.evm, env);
        }

        let block_number = env.block.number.to::<u64>();
        self.inner.spec_id = env.spec.into();
        self.inner.base_block_reward = base_block_reward(self.chain_spec.as_ref(), block_number);
        self.inner.ctx = segment.ctx.clone();
        self.inner.separate_block_gas = env.version.feature(evm2::EvmFeatures::EIP8037);
        self.inner.bal_index_offset = segment_idx as u64 * 2;
        self.inner.cumulative_gas_used = 0;
        self.inner.block_regular_gas_used = 0;
        self.inner.block_state_gas_used = 0;
        self.inner.blob_gas_used = 0;
    }

    fn reseed_block_hashes(&mut self, block_number: u64) {
        for (number, hash) in self.plan.hashes_for_block(block_number) {
            let number = evm2::interpreter::Word::from(number);
            self.inner.evm.overlay_db_mut().insert_block_hash(&number, &hash);
        }
    }

    fn initialize(&mut self) -> Result<(), BlockExecutionError> {
        if self.initialized {
            return Ok(())
        }

        let raw_index = self.inner.evm.state().bal_index().get();
        let segment_idx = if raw_index == 0 {
            0
        } else {
            let tx_index = (raw_index - 1) as usize;
            if tx_index >= self.plan.transaction_count {
                return Err(BlockExecutionError::msg("BAL index is outside big-block transactions"))
            }
            let segment_idx = self.plan.segment_index_for_tx(tx_index);
            self.worker_segment = Some(segment_idx);
            self.inner
                .evm
                .state_mut()
                .set_bal_index(BlockAccessIndex::new(raw_index + segment_idx as u64 * 2));
            self.next_segment = self.plan.segments.len();
            segment_idx
        };

        self.configure_segment(segment_idx);
        self.reseed_block_hashes(self.plan.segments[segment_idx].evm_env.block.number.to::<u64>());
        self.initialized = true;
        Ok(())
    }

    fn finish_segment(&mut self) -> Result<FinishedBigBlockSegment, BlockExecutionError> {
        self.inner.set_transaction_block_access_index();
        let context = EthBlockExecutor::<F::Types>::block_context(
            self.inner.deposit_contract_address,
            self.inner.ctx.parent_hash,
            self.inner.ctx.parent_beacon_block_root,
            self.inner.ctx.ommers,
            None,
        );
        let receipts = &self.inner.receipts[self.segment_receipt_start..];
        let mut requests = block_requests_from_receipts(self.inner.spec_id, context, receipts)?;
        let stream_hashed_state = self.inner.hashed_state_update_hook.is_some();
        post_execution_system_call_state_changes(
            &mut self.inner.evm,
            &mut self.inner.block_state,
            stream_hashed_state,
            &mut |state| emit_hashed_state(&mut self.inner.hashed_state_update_hook, state),
            self.inner.spec_id,
            context,
            &mut requests,
        )
        .map_err(BlockExecutionError::from)?;

        let withdrawals = self.inner.ctx.withdrawals.clone();
        let context = EthBlockExecutor::<F::Types>::block_context(
            self.inner.deposit_contract_address,
            self.inner.ctx.parent_hash,
            self.inner.ctx.parent_beacon_block_root,
            self.inner.ctx.ommers,
            withdrawals.as_deref(),
        );
        let block_number = self.inner.evm.block_env().number.to::<u64>();
        let block_beneficiary = self.inner.evm.block_env().beneficiary;
        post_block_balance_state_changes(
            &mut self.inner.evm,
            &mut self.inner.block_state,
            stream_hashed_state,
            &mut |state| emit_hashed_state(&mut self.inner.hashed_state_update_hook, state),
            self.inner.base_block_reward,
            block_number,
            block_beneficiary,
            context.ommers,
            context.withdrawals,
        )
        .map_err(BlockExecutionError::from)?;

        Ok(FinishedBigBlockSegment {
            state: core::mem::take(&mut self.inner.block_state),
            requests,
            gas_used: final_block_gas_used(
                self.inner.separate_block_gas,
                self.inner.cumulative_gas_used,
                self.inner.block_regular_gas_used,
                self.inner.block_state_gas_used,
            ),
            blob_gas_used: self.inner.blob_gas_used,
        })
    }

    fn merge_segment(&mut self, segment: FinishedBigBlockSegment) {
        let Ok(()) = segment.state.visit(&mut self.state);
        self.requests.extend(segment.requests);
        self.gas_used_offset += segment.gas_used;
        self.blob_gas_used_offset += segment.blob_gas_used;
        self.inner.cumulative_gas_used = 0;
        self.inner.block_regular_gas_used = 0;
        self.inner.block_state_gas_used = 0;
        self.inner.blob_gas_used = 0;
    }

    fn after_committed_transaction(&mut self) -> Result<(), BlockExecutionError> {
        if let Some(receipt) = self.inner.receipts.last_mut() {
            receipt.cumulative_gas_used += self.gas_used_offset;
        }
        self.tx_counter += 1;
        while self.worker_segment.is_none() &&
            self.next_segment < self.plan.segments.len() &&
            self.tx_counter == self.plan.segments[self.next_segment].start_tx
        {
            self.apply_segment_boundary()?;
        }
        Ok(())
    }

    fn apply_segment_boundary(&mut self) -> Result<(), BlockExecutionError> {
        let segment = self.finish_segment()?;
        self.merge_segment(segment);
        self.inner.evm.state_mut().bump_bal_index();

        let segment_idx = self.next_segment;
        self.next_segment += 1;
        self.segment_receipt_start = self.inner.receipts.len();
        self.configure_segment(segment_idx);
        self.reseed_block_hashes(self.plan.segments[segment_idx].evm_env.block.number.to::<u64>());
        self.inner.apply_pre_execution_changes()
    }
}

impl<'a, C, F> BlockExecutor for EthBigBlockExecutor<'a, C, F>
where
    C: EthereumHardforks,
    F: EvmFactory,
    F::Types: EvmTypes<Tx = TxEnvelope>,
    <F::Types as evm2::EvmTypesHost>::Tx: Typed2718,
    <F::Types as evm2::EvmTypesHost>::TxResultExt: Send,
{
    type Transaction = TransactionSigned;
    type Receipt = Receipt;
    type Evm = Evm<'a, F::Types>;
    type TransactionResultWithState = EthTransactionResultWithState<F::Types>;
    type BlockAccessList = Bal;

    fn evm(&self) -> &Self::Evm {
        self.inner.evm()
    }

    fn evm_mut(&mut self) -> &mut Self::Evm {
        self.inner.evm_mut()
    }

    fn set_state_hook(&mut self, hook: impl FnMut(HashedPostState) + Send + 'static) -> bool {
        self.inner.set_state_hook(hook)
    }

    fn convert_block_access_list(
        block_access_list: &BlockAccessList,
    ) -> Result<Self::BlockAccessList, BlockExecutionError> {
        EthBlockExecutor::<F::Types>::convert_block_access_list(block_access_list)
    }

    fn set_block_access_list(&mut self, block_access_list: Arc<Self::BlockAccessList>) {
        self.inner.set_block_access_list(block_access_list);
    }

    fn set_block_access_index(&mut self, index: BlockAccessIndex) {
        let index = if let Some(current_segment) = self.worker_segment {
            let raw_index = index.get();
            if let Some(tx_index) = raw_index.checked_sub(1).map(|index| index as usize) {
                if tx_index < self.plan.transaction_count {
                    let segment = self.plan.segment_index_for_tx(tx_index);
                    if segment != current_segment {
                        self.configure_segment(segment);
                        self.reseed_block_hashes(
                            self.plan.segments[segment].evm_env.block.number.to::<u64>(),
                        );
                    }
                    self.worker_segment = Some(segment);
                    BlockAccessIndex::new(raw_index + segment as u64 * 2)
                } else {
                    index
                }
            } else {
                index
            }
        } else {
            index
        };
        self.inner.set_block_access_index(index);
    }

    fn enable_block_access_list_builder(&mut self) {
        self.inner.enable_block_access_list_builder();
    }

    fn take_block_access_list(&mut self) -> Option<BlockAccessList> {
        self.inner.take_block_access_list()
    }

    fn apply_pre_execution_changes(&mut self) -> Result<(), BlockExecutionError> {
        self.initialize()?;
        self.inner.apply_pre_execution_changes()?;
        while self.worker_segment.is_none() &&
            self.next_segment < self.plan.segments.len() &&
            self.tx_counter == self.plan.segments[self.next_segment].start_tx
        {
            self.apply_segment_boundary()?;
        }
        Ok(())
    }

    fn execute_transaction_without_commit(
        &mut self,
        transaction: impl ExecutorTx<Self>,
    ) -> Result<Self::TransactionResultWithState, BlockExecutionError> {
        self.initialize()?;
        self.inner.execute_transaction_without_commit(transaction)
    }

    fn commit_transaction(
        &mut self,
        output: Self::TransactionResultWithState,
    ) -> Result<GasOutput, BlockExecutionError> {
        self.initialize()?;
        let output = self.inner.commit_transaction(output)?;
        self.after_committed_transaction()?;
        Ok(output)
    }

    fn receipts(&self) -> &[Receipt] {
        self.inner.receipts()
    }

    fn finish_with_block_access_list(
        mut self,
    ) -> Result<(BlockExecutionOutput<Receipt>, Option<BlockAccessList>), BlockExecutionError> {
        self.initialize()?;
        let segment = self.finish_segment()?;
        self.merge_segment(segment);
        let block_access_list = self.inner.take_block_access_list();
        let result = reth_execution_types::BlockExecutionResult {
            receipts: core::mem::take(&mut self.inner.receipts),
            requests: self.requests,
            gas_used: self.gas_used_offset,
            blob_gas_used: self.blob_gas_used_offset,
        };
        Ok((BlockExecutionOutput::new(result, self.state), block_access_list))
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
    use super::{
        final_block_gas_used, map_transaction_execution_error, EthBigBlockPlan, EthBigBlockSegment,
        EthBlockExecutionCtx, EthEvmEnv,
    };
    use crate::execution::EthExecutionError;
    use alloy_consensus::{SignableTransaction, TxLegacy};
    use alloy_eip7928::BlockAccessIndex;
    use alloy_primitives::{address, Address, Bytes, Signature, TxKind, B256, U256};
    use evm2::{
        bytecode::Bytecode,
        env::BlockEnv,
        evm::{AccountInfo, Database, Db},
        interpreter::Word,
        SpecId,
    };
    use reth_chainspec::{Chain, ChainSpecBuilder};
    use reth_ethereum_primitives::TransactionSigned;
    use reth_evm::{
        BlockExecutionError, BlockExecutor, BlockExecutorFactory, BlockValidationError,
        CommitChanges,
    };
    use reth_primitives_traits::Recovered;
    use std::{collections::BTreeMap, convert::Infallible, sync::Arc};

    fn segment(
        start_tx: usize,
        block_number: u64,
        parent_hash: B256,
    ) -> EthBigBlockSegment<'static> {
        let block = BlockEnv { number: U256::from(block_number), ..Default::default() };
        EthBigBlockSegment {
            start_tx,
            evm_env: EthEvmEnv { block, ..EthEvmEnv::default() },
            ctx: EthBlockExecutionCtx {
                tx_count_hint: Some(1),
                parent_hash,
                parent_beacon_block_root: None,
                ommers: &[],
                withdrawals: None,
                extra_data: Bytes::new(),
                slot_number: None,
            },
        }
    }

    #[test]
    fn big_block_plan_tracks_segment_boundaries_and_hashes() {
        let plan = EthBigBlockPlan::new(
            vec![segment(0, 10, B256::with_last_byte(1)), segment(2, 11, B256::with_last_byte(2))],
            vec![(8, B256::with_last_byte(8)), (9, B256::with_last_byte(9))],
            3,
        );

        assert_eq!(plan.transaction_count, 3);
        assert_eq!(plan.segment_index_for_tx(0), 0);
        assert_eq!(plan.segment_index_for_tx(1), 0);
        assert_eq!(plan.segment_index_for_tx(2), 1);
        assert_eq!(
            plan.block_hashes,
            vec![
                (8, B256::with_last_byte(8)),
                (9, B256::with_last_byte(9)),
                (10, B256::with_last_byte(2)),
            ]
        );
    }

    #[derive(Default)]
    struct TestDatabase {
        accounts: BTreeMap<Address, AccountInfo>,
    }

    impl Database for TestDatabase {
        type Error = Infallible;

        fn get_account(&mut self, address: &Address) -> Result<Option<AccountInfo>, Self::Error> {
            Ok(self.accounts.get(address).cloned())
        }

        fn get_code_by_hash(&mut self, _code_hash: &B256) -> Result<Bytecode, Self::Error> {
            Ok(Bytecode::default())
        }

        fn get_storage(&mut self, _address: &Address, _key: &Word) -> Result<Word, Self::Error> {
            Ok(Word::ZERO)
        }

        fn get_block_hash(&mut self, _number: &Word) -> Result<Option<B256>, Self::Error> {
            Ok(None)
        }
    }

    fn transfer(from: Address, to: Address, nonce: u64) -> Recovered<TransactionSigned> {
        let tx = TransactionSigned::Legacy(
            TxLegacy {
                chain_id: Some(1),
                nonce,
                gas_limit: 21_000,
                gas_price: 1,
                to: TxKind::Call(to),
                value: U256::from(1),
                ..Default::default()
            }
            .into_signed(Signature::test_signature()),
        );
        Recovered::new_unchecked(tx, from)
    }

    #[test]
    fn big_block_executor_preserves_state_across_segment_switch() {
        let from = address!("0000000000000000000000000000000000000001");
        let first_target = address!("0000000000000000000000000000000000000010");
        let second_target = address!("0000000000000000000000000000000000000020");
        let mut database = TestDatabase::default();
        database
            .accounts
            .insert(from, AccountInfo::default().with_balance(U256::from(1_000_000u64)));

        let mut first = segment(0, 1, B256::with_last_byte(1));
        first.evm_env = EthEvmEnv::new(SpecId::FRONTIER, first.evm_env.block, 1);
        let mut second = segment(1, 2, B256::with_last_byte(2));
        second.evm_env = EthEvmEnv::new(SpecId::LONDON, second.evm_env.block, 1);
        let plan = EthBigBlockPlan::new(vec![first, second], Vec::new(), 2);
        let chain_spec =
            ChainSpecBuilder::mainnet().chain(Chain::mainnet()).paris_activated().build();
        let factory = super::super::factory::EthBigBlockExecutorFactory::new(
            super::super::factory::EthBlockExecutorFactory::new(Arc::new(chain_spec)),
        );
        let evm = factory.evm_with_env(Db::new(database), plan.segments[0].evm_env.clone());
        let mut executor = factory.create_executor(evm, plan);

        executor.apply_pre_execution_changes().expect("first segment pre-execution");
        executor
            .execute_transaction_with_commit_condition(transfer(from, first_target, 0), |_| {
                CommitChanges::Yes
            })
            .expect("first segment transaction")
            .expect("transaction committed");
        executor
            .execute_transaction_with_commit_condition(transfer(from, second_target, 1), |_| {
                CommitChanges::Yes
            })
            .expect("second segment transaction")
            .expect("transaction committed");
        let (output, _) = executor.finish_with_block_access_list().expect("finish big block");

        assert_eq!(output.result.receipts.len(), 2);
        assert_eq!(output.result.gas_used, 42_000);
        assert_eq!(output.account(&first_target).unwrap().unwrap().balance, U256::from(1));
        assert_eq!(output.account(&second_target).unwrap().unwrap().balance, U256::from(1));
    }

    #[test]
    fn big_block_worker_switches_segments_from_bal_index() {
        let from = address!("0000000000000000000000000000000000000001");
        let target = address!("0000000000000000000000000000000000000010");
        let mut database = TestDatabase::default();
        database
            .accounts
            .insert(from, AccountInfo::default().with_balance(U256::from(1_000_000u64)));

        let mut first = segment(0, 1, B256::with_last_byte(1));
        first.evm_env = EthEvmEnv::new(SpecId::FRONTIER, first.evm_env.block, 1);
        let mut second = segment(1, 2, B256::with_last_byte(2));
        second.evm_env = EthEvmEnv::new(SpecId::LONDON, second.evm_env.block, 1);
        let plan = EthBigBlockPlan::new(vec![first, second], Vec::new(), 2);
        let chain_spec =
            ChainSpecBuilder::mainnet().chain(Chain::mainnet()).paris_activated().build();
        let factory = super::super::factory::EthBigBlockExecutorFactory::new(
            super::super::factory::EthBlockExecutorFactory::new(Arc::new(chain_spec)),
        );
        let evm = factory.evm_with_env(Db::new(database), plan.segments[0].evm_env.clone());
        let mut executor = factory.create_executor(evm, plan);

        executor.set_block_access_index(BlockAccessIndex::new(2));
        executor
            .execute_transaction_without_commit(transfer(from, target, 0))
            .expect("worker transaction in second segment");
        assert_eq!(executor.evm().block().number, U256::from(2));

        executor.set_block_access_index(BlockAccessIndex::new(1));
        executor
            .execute_transaction_without_commit(transfer(from, target, 0))
            .expect("worker transaction in first segment");
        assert_eq!(executor.evm().block().number, U256::from(1));
    }

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
