//! EVM-backed Ethereum executor.
//!
//! [`EthBlockExecutor`] wraps evm2's [`evm2::block::ethereum::EthBlockExecutor`], which implements
//! Ethereum block execution, and adapts it to reth's [`BlockExecutor`] trait.

use reth_execution_types::{BlockState, StateUpdate};

use crate::{
    factory::{EthBlockExecutorFactory, EvmFactory},
    EthBlockExecutionCtx, EthEvmEnv, RethReceiptBuilder,
};
use alloc::{borrow::Cow, sync::Arc, vec::Vec};
use alloy_consensus::transaction::TransactionEnvelope;
use alloy_eip7928::{BlockAccessIndex, BlockAccessList};
use alloy_eips::{eip2718::Typed2718, eip7685::Requests};
use alloy_primitives::B256;
use evm2::{
    block::{
        ethereum::{self as evm2_eth, EthBlockRules, EthPostExecutionOutput},
        BlockExecutor as _,
    },
    ethereum::TxEnvelope,
    evm::Bal,
    interpreter::Host,
    BaseEvmTypes, Evm, EvmTypes,
};
use reth_chainspec::EthChainSpec;
use reth_ethereum_forks::EthereumHardforks;
use reth_ethereum_primitives::{Receipt, TransactionSigned};
use reth_evm::{
    BlockExecutionError, BlockExecutionOutput, BlockExecutor, ExecutorTx, GasOutput,
    ReceiptBuilder, ReceiptBuilderCtx, RecoveredTx,
};

pub use evm2::block::ethereum::EthTransactionResultWithState;

/// Adapts a reth [`ReceiptBuilder`] to evm2's receipt builder.
#[derive(Debug, Clone, Copy, Default)]
pub struct ReceiptBuilderAdapter<R>(pub R);

impl<R: ReceiptBuilder> evm2::block::ReceiptBuilder for ReceiptBuilderAdapter<R> {
    type Transaction = R::Transaction;
    type Receipt = R::Receipt;

    fn build_receipt<T: EvmTypes>(
        &self,
        ctx: ReceiptBuilderCtx<<R::Transaction as TransactionEnvelope>::TxType, evm2::TxResult<T>>,
    ) -> R::Receipt {
        self.0.build_receipt::<T>(ctx)
    }
}

/// Configured Ethereum block executor backed by evm2.
#[expect(missing_debug_implementations)]
pub struct EthBlockExecutor<'a, T = BaseEvmTypes, R = RethReceiptBuilder>
where
    T: EvmTypes<Tx: Typed2718, TxResultExt: Send + Sync>,
    R: ReceiptBuilder,
{
    /// evm2 executor that runs the block.
    pub inner: evm2_eth::EthBlockExecutor<'a, T, ReceiptBuilderAdapter<R>>,
    ctx: EthBlockExecutionCtx<'a>,
}

/// Converts reth's execution context into evm2's.
fn evm2_ctx<'a>(ctx: &EthBlockExecutionCtx<'a>) -> evm2_eth::EthBlockExecutionCtx<'a> {
    evm2_eth::EthBlockExecutionCtx {
        tx_count_hint: ctx.tx_count_hint,
        parent_hash: ctx.parent_hash,
        parent_beacon_block_root: ctx.parent_beacon_block_root,
        ommers: Cow::Borrowed(ctx.ommers),
        withdrawals: ctx.withdrawals.clone(),
    }
}

impl<'a, T, R> EthBlockExecutor<'a, T, R>
where
    T: EvmTypes<Tx: Typed2718, TxResultExt: Send + Sync>,
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
        let block_number = evm.block_env().number.to::<u64>();
        let rules = EthBlockRules::new(
            chain_spec,
            block_number,
            chain_spec.deposit_contract().map(|contract| contract.address),
        );
        let inner = evm2_eth::EthBlockExecutor::new(
            evm,
            evm2_ctx(&ctx),
            rules,
            ReceiptBuilderAdapter(receipt_builder),
        );
        Self { inner, ctx }
    }

    /// Returns the block execution context.
    pub const fn context(&self) -> &EthBlockExecutionCtx<'a> {
        &self.ctx
    }

    /// Returns the accumulated block state.
    pub const fn block_state(&self) -> &BlockState {
        self.inner.block_state()
    }

    /// Commits detached state without recording a transaction receipt or gas usage.
    pub fn commit_pending_state(&mut self, state: &evm2::evm::PendingState) {
        self.inner.commit_pending_state(state);
    }

    /// Sets the Ethereum fork used for block system calls and deposit request processing.
    pub const fn set_spec_id(&mut self, spec_id: evm2::SpecId) {
        self.inner.set_spec_id(spec_id);
    }
}

impl<'a, T, R> BlockExecutor for EthBlockExecutor<'a, T, R>
where
    T: EvmTypes<Tx: Typed2718, TxResultExt: Send + Sync>,
    R: ReceiptBuilder,
{
    type Transaction = R::Transaction;
    type Receipt = R::Receipt;
    type Evm = Evm<'a, T>;
    type TransactionResultWithState =
        EthTransactionResultWithState<T, <R::Transaction as TransactionEnvelope>::TxType>;
    type BlockAccessList = Bal;

    fn evm(&self) -> &Self::Evm {
        &self.inner.evm
    }

    fn evm_mut(&mut self) -> &mut Self::Evm {
        &mut self.inner.evm
    }

    fn set_state_hook(&mut self, hook: impl FnMut(StateUpdate) + Send + 'static) -> bool {
        self.inner.set_state_hook(hook)
    }

    fn convert_block_access_list(
        block_access_list: &BlockAccessList,
    ) -> Result<Self::BlockAccessList, BlockExecutionError> {
        Bal::try_from(block_access_list.as_slice()).map_err(BlockExecutionError::other)
    }

    fn set_block_access_list(&mut self, block_access_list: Arc<Self::BlockAccessList>) {
        self.inner.set_block_access_list(block_access_list);
    }

    fn set_block_access_index(&mut self, index: BlockAccessIndex) {
        self.inner.set_block_access_index(index);
    }

    fn enable_block_access_list_builder(&mut self) {
        self.inner.enable_block_access_list_builder();
    }

    fn take_block_access_list(&mut self) -> Option<BlockAccessList> {
        self.inner.take_block_access_list().map(Into::into)
    }

    fn apply_pre_execution_changes(&mut self) -> Result<(), BlockExecutionError> {
        self.inner.apply_pre_execution_changes()
    }

    fn into_state(self) -> reth_evm::BundleState {
        self.inner.into_state()
    }

    fn validate_transaction_gas_limit(
        &mut self,
        transaction_gas_limit: u64,
    ) -> Result<(), BlockExecutionError> {
        self.inner.validate_transaction_gas_limit(transaction_gas_limit)
    }

    fn execute_transaction_with_commit_condition(
        &mut self,
        transaction: impl ExecutorTx<Self>,
        commit: impl FnOnce(&evm2::TxResult<T>, evm2::TxStateChanges<'_>) -> reth_evm::CommitChanges,
    ) -> Result<Option<GasOutput>, BlockExecutionError> {
        let (transaction, tx) = transaction.into_parts();
        self.inner.execute_transaction_with_commit_condition((transaction, tx.tx()), commit)
    }

    fn execute_transaction_without_commit(
        &mut self,
        transaction: impl ExecutorTx<Self>,
    ) -> Result<Self::TransactionResultWithState, BlockExecutionError> {
        let (transaction, tx) = transaction.into_parts();
        self.inner.execute_transaction_without_commit((transaction, tx.tx()))
    }

    fn commit_transaction(
        &mut self,
        output: Self::TransactionResultWithState,
    ) -> Result<GasOutput, BlockExecutionError> {
        self.inner.commit_transaction(output)
    }

    fn receipts(&self) -> &[Self::Receipt] {
        &self.inner.receipts
    }

    fn finish_with_block_access_list(
        self,
    ) -> Result<(BlockExecutionOutput<Self::Receipt>, Option<BlockAccessList>), BlockExecutionError>
    {
        let (output, bal) = self.finish_with_prepared_block_access_list()?;
        Ok((output, bal.map(Into::into)))
    }

    fn finish_with_prepared_block_access_list(
        self,
    ) -> Result<(BlockExecutionOutput<Self::Receipt>, Option<Bal>), BlockExecutionError> {
        let (output, bal) = self.inner.finish_with_block_access_list()?;
        Ok((output.into(), bal))
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
    <F::Types as evm2::EvmTypesHost>::TxResultExt: Send + Sync,
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
    requests: Requests,
    gas_used_offset: u64,
    receipt_gas_used_offset: u64,
    blob_gas_used_offset: u64,
}

impl<'a, C, F> EthBigBlockExecutor<'a, C, F>
where
    C: EthereumHardforks,
    F: EvmFactory,
    F::Types: EvmTypes<Tx = TxEnvelope>,
    <F::Types as evm2::EvmTypesHost>::Tx: Typed2718,
    <F::Types as evm2::EvmTypesHost>::TxResultExt: Send + Sync,
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
            requests: Requests::default(),
            gas_used_offset: 0,
            receipt_gas_used_offset: 0,
            blob_gas_used_offset: 0,
        }
    }

    fn configure_segment(&mut self, segment_idx: usize) {
        let segment = &self.plan.segments[segment_idx];
        let env = &segment.evm_env;
        // The version carries more than the spec and features (blob cap, gas caps, code limits), so
        // install the whole segment config.
        self.factory.reconfigure_evm(&mut self.inner.inner.evm, env);

        let block_number = env.block.number.to::<u64>();
        let rules = EthBlockRules::new(
            self.chain_spec.as_ref(),
            block_number,
            self.inner.inner.rules().deposit_contract_address,
        );
        self.inner.ctx = segment.ctx.clone();
        self.inner.inner.start_block(evm2_ctx(&segment.ctx), rules, segment_idx as u64 * 2);
    }

    fn reseed_block_hashes(&mut self, block_number: u64) {
        for (number, hash) in self.plan.hashes_for_block(block_number) {
            let number = evm2::interpreter::Word::from(number);
            self.inner.inner.evm.overlay_db_mut().insert_block_hash(&number, &hash);
        }
    }

    fn initialize(&mut self) -> Result<(), BlockExecutionError> {
        if self.initialized {
            return Ok(())
        }

        let raw_index = self.inner.inner.evm.state().bal_index().get();
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
                .inner
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
        let EthPostExecutionOutput { requests, gas_used, blob_gas_used } =
            self.inner.inner.apply_post_execution_changes(self.segment_receipt_start)?;
        Ok(FinishedBigBlockSegment { requests, gas_used, blob_gas_used })
    }

    fn merge_segment(&mut self, segment: FinishedBigBlockSegment) {
        self.requests.extend(segment.requests);
        self.gas_used_offset += segment.gas_used;
        self.receipt_gas_used_offset += self.inner.inner.cumulative_gas_used;
        self.blob_gas_used_offset += segment.blob_gas_used;
        self.inner.inner.reset_gas_used();
    }

    fn after_committed_transaction(&mut self) -> Result<(), BlockExecutionError> {
        if let Some(receipt) = self.inner.inner.receipts.last_mut() {
            receipt.cumulative_gas_used += self.receipt_gas_used_offset;
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
        self.inner.inner.evm.state_mut().bump_bal_index();

        let segment_idx = self.next_segment;
        self.next_segment += 1;
        self.segment_receipt_start = self.inner.inner.receipts.len();
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
    <F::Types as evm2::EvmTypesHost>::TxResultExt: Send + Sync,
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

    fn set_state_hook(&mut self, hook: impl FnMut(StateUpdate) + Send + 'static) -> bool {
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

    fn into_state(self) -> reth_evm::BundleState {
        self.inner.into_state()
    }

    fn validate_transaction_gas_limit(
        &mut self,
        gas_limit: u64,
    ) -> Result<(), BlockExecutionError> {
        self.inner.validate_transaction_gas_limit(gas_limit)
    }

    fn execute_transaction_with_commit_condition(
        &mut self,
        transaction: impl ExecutorTx<Self>,
        commit: impl FnOnce(
            &evm2::TxResult<F::Types>,
            evm2::TxStateChanges<'_>,
        ) -> reth_evm::CommitChanges,
    ) -> Result<Option<GasOutput>, BlockExecutionError> {
        self.initialize()?;
        let output = self.inner.execute_transaction_with_commit_condition(transaction, commit)?;
        if output.is_some() {
            self.after_committed_transaction()?;
        }
        Ok(output)
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
        self.inner.inner.receipts()
    }

    fn finish_with_block_access_list(
        self,
    ) -> Result<(BlockExecutionOutput<Receipt>, Option<BlockAccessList>), BlockExecutionError> {
        let (output, bal) = self.finish_with_prepared_block_access_list()?;
        Ok((output, bal.map(Into::into)))
    }

    fn finish_with_prepared_block_access_list(
        mut self,
    ) -> Result<(BlockExecutionOutput<Receipt>, Option<Bal>), BlockExecutionError> {
        self.initialize()?;
        let segment = self.finish_segment()?;
        self.merge_segment(segment);
        let block_access_list = self.inner.inner.evm.state_mut().take_bal_builder();
        let result = reth_execution_types::BlockExecutionResult {
            receipts: core::mem::take(&mut self.inner.inner.receipts),
            requests: self.requests,
            gas_used: self.gas_used_offset,
            blob_gas_used: self.blob_gas_used_offset,
        };
        Ok((
            BlockExecutionOutput::new(result, self.inner.inner.block_state.into_bundle()),
            block_access_list,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dao_fork;
    use alloy_consensus::{SignableTransaction, TxEip4844, TxLegacy};
    use alloy_eip7928::BlockAccessIndex;
    use alloy_primitives::{address, Address, Bytes, Signature, TxKind, B256, U256};
    use evm2::{
        block::ethereum::final_block_gas_used,
        bytecode::Bytecode,
        env::BlockEnv,
        evm::{
            AccountInfo, Database, Db, BUILDER_DEPOSIT_REQUEST_ADDRESS,
            BUILDER_EXIT_REQUEST_ADDRESS, CONSOLIDATION_REQUEST_ADDRESS,
            WITHDRAWAL_REQUEST_ADDRESS,
        },
        interpreter::Word,
        BaseEvmTypes, SpecId,
    };
    use reth_chainspec::{Chain, ChainSpecBuilder, EthereumHardfork, ForkCondition};
    use reth_ethereum_primitives::TransactionSigned;
    use reth_evm::{BlockExecutor, BlockExecutorFactory, CommitChanges};
    use reth_primitives_traits::Recovered;
    use std::{collections::BTreeMap, convert::Infallible, sync::Arc};

    fn segment(
        start_tx: usize,
        block_number: u64,
        parent_hash: B256,
    ) -> EthBigBlockSegment<'static> {
        let block =
            BlockEnv::<BaseEvmTypes> { number: U256::from(block_number), ..Default::default() };
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

        fn get_block_hash(&mut self, _number: &Word) -> Result<B256, Self::Error> {
            Ok(B256::ZERO)
        }
    }

    /// Deploys no-op request contracts, which blocks from Prague on call.
    fn insert_request_contracts(database: &mut TestDatabase) {
        let code = Bytecode::new_legacy(Bytes::from_static(&[0x00]));
        for address in [
            WITHDRAWAL_REQUEST_ADDRESS,
            CONSOLIDATION_REQUEST_ADDRESS,
            BUILDER_DEPOSIT_REQUEST_ADDRESS,
            BUILDER_EXIT_REQUEST_ADDRESS,
        ] {
            database.accounts.insert(address, AccountInfo::default().with_code(code.clone()));
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
        first.evm_env = EthEvmEnv::new(SpecId::LONDON, first.evm_env.block, 1);
        first.evm_env.block.basefee = U256::from(1);
        let mut second = segment(1, 2, B256::with_last_byte(2));
        second.evm_env = EthEvmEnv::new(SpecId::SHANGHAI, second.evm_env.block, 1);
        second.evm_env.block.basefee = U256::from(1);
        let plan = EthBigBlockPlan::new(vec![first, second], Vec::new(), 2);
        let chain_spec =
            ChainSpecBuilder::mainnet().chain(Chain::mainnet()).paris_activated().build();
        let factory = super::super::factory::EthBigBlockExecutorFactory::new(
            super::super::factory::EthBlockExecutorFactory::new(Arc::new(chain_spec)),
        );
        let mut env = plan.segments[0].evm_env.clone();
        env.version.features.remove(evm2::EvmFeatures::BASE_FEE_CHECK);
        let evm = factory.evm_with_env(Db::new(database), env);
        let mut executor = factory.create_executor(evm, plan);

        executor.apply_pre_execution_changes().expect("first segment pre-execution");
        assert!(executor
            .execute_transaction_with_commit_condition(
                transfer(from, first_target, 0),
                |_, changes| {
                    // The callback sees the uncommitted state changes before deciding.
                    assert_eq!(changes.account_info(&from).expect("sender changed").nonce, 1);
                    CommitChanges::No
                }
            )
            .expect("discarded transaction")
            .is_none());
        executor
            .execute_transaction_with_commit_condition(transfer(from, first_target, 0), |_, _| {
                CommitChanges::Yes
            })
            .expect("first segment transaction")
            .expect("transaction committed");
        executor
            .execute_transaction_with_commit_condition(transfer(from, second_target, 1), |_, _| {
                CommitChanges::Yes
            })
            .expect("second segment transaction")
            .expect("transaction committed");
        let (output, _) = executor.finish_with_block_access_list().expect("finish big block");

        assert_eq!(output.result.receipts.len(), 2);
        assert_eq!(output.result.gas_used, 42_000);
        assert_eq!(output.account(&first_target).unwrap().unwrap().balance, U256::from(1));
        assert_eq!(output.account(&second_target).unwrap().unwrap().balance, U256::from(1));
        // The prewarming environment must not change the beneficiary reward during execution.
        assert_eq!(
            output
                .account(&Address::ZERO)
                .flatten()
                .map(|account| account.balance)
                .unwrap_or_default(),
            U256::ZERO
        );
    }

    #[test]
    fn amsterdam_admission_enforces_uncapped_state_gas_budget() {
        let factory = super::super::factory::EthBlockExecutorFactory::new(Arc::new(
            ChainSpecBuilder::mainnet().amsterdam_activated().build(),
        ));
        let ctx = segment(0, 1, B256::ZERO).ctx;
        for cap in [u64::MAX, 400_000] {
            let mut env = EthEvmEnv::new(
                SpecId::AMSTERDAM,
                BlockEnv::<BaseEvmTypes> { gas_limit: U256::from(1_000_000), ..Default::default() },
                1,
            );
            env.version.tx_gas_limit_cap = cap;
            let evm = factory.evm_with_env(Db::new(TestDatabase::default()), env);
            let mut executor = factory.create_executor(evm, ctx.clone());
            executor.inner.block_state_gas_used = 600_000;
            executor.inner.block_regular_gas_used = 0;
            assert!(executor.validate_transaction_gas_limit(500_000).is_err());
            assert!(executor.validate_transaction_gas_limit(400_000).is_ok());
        }
    }

    #[test]
    fn pre_london_creation_preserves_legacy_ef01_code() {
        let sender = Address::with_last_byte(1);
        let mut database = TestDatabase::default();
        database
            .accounts
            .insert(sender, AccountInfo::default().with_balance(U256::from(1_000_000)));
        let factory = super::super::factory::EthBlockExecutorFactory::new(Arc::new(
            ChainSpecBuilder::mainnet().berlin_activated().build(),
        ));
        let env = EthEvmEnv::new(
            SpecId::BERLIN,
            BlockEnv::<BaseEvmTypes> { gas_limit: U256::from(1_000_000), ..Default::default() },
            1,
        );
        let evm = factory.evm_with_env(Db::new(database), env);
        let mut executor = factory.create_executor(evm, segment(0, 1, B256::ZERO).ctx);
        let tx = TransactionSigned::Legacy(
            TxLegacy {
                chain_id: Some(1),
                gas_limit: 100_000,
                gas_price: 1,
                to: TxKind::Create,
                // Return the two-byte runtime code 0xEF01 from initcode.
                input: alloy_primitives::bytes!("61ef016000526002601ef3"),
                ..Default::default()
            }
            .into_signed(Signature::test_signature()),
        );
        executor.apply_pre_execution_changes().unwrap();
        executor.execute_transaction(Recovered::new_unchecked(tx, sender)).unwrap();
        let (output, _) = executor.finish_with_block_access_list().unwrap();
        let code = &output.state.contracts[&alloy_primitives::keccak256([0xef, 0x01])];
        assert!(code.is_legacy());
        assert_eq!(code.original_bytes().as_ref(), &[0xef, 0x01]);
    }

    #[test]
    fn executor_applies_dao_fork_balance_transfer_before_rewards() {
        let dao_account = address!("d4fe7bc31cedb7bfb8a345f31e668033056b2728");
        let beneficiary = address!("bf4ed7b27f1d666546e30d74d50d173d20bca754");
        let mut database = TestDatabase::default();
        database.accounts.insert(dao_account, AccountInfo::default().with_balance(U256::from(7)));

        let chain_spec = ChainSpecBuilder::mainnet()
            .with_fork(EthereumHardfork::Dao, ForkCondition::Block(1))
            .build();
        let factory = super::super::factory::EthBlockExecutorFactory::new(Arc::new(chain_spec));
        let env = EthEvmEnv::new(
            SpecId::FRONTIER,
            BlockEnv::<BaseEvmTypes> {
                number: U256::from(1),
                gas_limit: U256::from(30_000),
                beneficiary: dao_account,
                ..Default::default()
            },
            1,
        );
        let evm = factory.evm_with_env(Db::new(database), env);
        let mut executor = factory.create_executor(
            evm,
            EthBlockExecutionCtx {
                tx_count_hint: Some(0),
                parent_hash: B256::ZERO,
                parent_beacon_block_root: None,
                ommers: &[],
                withdrawals: None,
                extra_data: Bytes::new(),
                slot_number: None,
            },
        );

        executor.apply_pre_execution_changes().expect("DAO block pre-execution succeeds");
        let (output, _) =
            executor.finish_with_block_access_list().expect("DAO block post-execution succeeds");

        assert_eq!(
            output.account(&dao_account).unwrap().unwrap().balance,
            U256::from(5 * alloy_consensus::constants::ETH_TO_WEI)
        );
        assert_eq!(output.account(&beneficiary).unwrap().unwrap().balance, U256::from(7));
    }

    #[test]
    fn dao_fork_drain_keeps_empty_accounts() {
        let [absent, empty, empty_with_balance, nonce_only, ..] = dao_fork::DAO_HARDFORK_ACCOUNTS;
        let beneficiary = dao_fork::DAO_HARDFORK_BENEFICIARY;

        let execute = |accounts: &[(Address, AccountInfo)]| {
            let mut database = TestDatabase::default();
            database.accounts.extend(accounts.iter().cloned());
            let chain_spec = ChainSpecBuilder::mainnet()
                .with_fork(EthereumHardfork::Dao, ForkCondition::Block(1))
                .build();
            let factory = super::super::factory::EthBlockExecutorFactory::new(Arc::new(chain_spec));
            let env = EthEvmEnv::new(
                SpecId::FRONTIER,
                BlockEnv::<BaseEvmTypes> {
                    number: U256::from(1),
                    gas_limit: U256::from(30_000),
                    beneficiary: Address::with_last_byte(0xbe),
                    ..Default::default()
                },
                1,
            );
            let evm = factory.evm_with_env(Db::new(database), env);
            let mut executor = factory.create_executor(evm, segment(0, 1, B256::ZERO).ctx);
            let (tx, rx) = std::sync::mpsc::channel();
            executor.set_state_hook(move |update| tx.send(update).unwrap());
            executor.apply_pre_execution_changes().unwrap();
            let output = executor.finish_with_block_access_list().unwrap().0;
            // The streamed updates hash to the same state as the finished bundle.
            let mut streamed = reth_trie_common::HashedPostState::default();
            for update in rx.try_iter() {
                streamed.extend(reth_execution_types::state_update_to_hashed_post_state(&update));
            }
            let bundled = reth_trie_common::HashedPostState::from_bundle_state::<
                reth_trie_common::KeccakKeyHasher,
            >(&output.state.state);
            assert_eq!(streamed.into_sorted(), bundled.into_sorted());
            output
        };

        // The fork predates EIP-161, so drained accounts stay, absent and empty ones included.
        let output = execute(&[
            (empty, AccountInfo::default()),
            (empty_with_balance, AccountInfo::default().with_balance(U256::from(7))),
            (nonce_only, AccountInfo { nonce: 1, balance: U256::from(3), ..Default::default() }),
        ]);
        assert_eq!(output.account(&absent).unwrap().unwrap().balance, U256::ZERO);
        assert_eq!(output.account(&empty), None);
        assert_eq!(output.account(&empty_with_balance).unwrap().unwrap().balance, U256::ZERO);
        let nonce_only_account = output.account(&nonce_only).unwrap().unwrap();
        assert_eq!((nonce_only_account.nonce, nonce_only_account.balance), (1, U256::ZERO));
        assert_eq!(output.account(&beneficiary).unwrap().unwrap().balance, U256::from(10));

        // Nothing is drained, and the empty beneficiary stays unchanged.
        let output = execute(&[(beneficiary, AccountInfo::default())]);
        assert_eq!(output.account(&beneficiary), None);
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
    fn amsterdam_segment_receipts_use_transaction_gas() {
        let sender = Address::with_last_byte(0x10);
        let mut database = TestDatabase::default();
        database.accounts.insert(
            sender,
            AccountInfo::default().with_balance(U256::from(1_000_000_000_000_000_000u64)),
        );
        insert_request_contracts(&mut database);
        let segments = (0..2)
            .map(|index| {
                let mut segment = segment(index, index as u64 + 1, B256::ZERO);
                segment.ctx.parent_beacon_block_root = Some(B256::ZERO);
                segment.evm_env = EthEvmEnv::new(
                    SpecId::AMSTERDAM,
                    BlockEnv::<BaseEvmTypes> {
                        number: U256::from(index + 1),
                        gas_limit: U256::from(30_000_000),
                        ..Default::default()
                    },
                    1,
                );
                segment
            })
            .collect();
        let plan = EthBigBlockPlan::new(segments, Vec::new(), 2);
        let factory = super::super::factory::EthBigBlockExecutorFactory::new(
            super::super::factory::EthBlockExecutorFactory::new(Arc::new(
                ChainSpecBuilder::mainnet().amsterdam_activated().build(),
            )),
        );
        let evm = factory.evm_with_env(Db::new(database), plan.segments[0].evm_env.clone());
        let mut executor = factory.create_executor(evm, plan);
        executor.apply_pre_execution_changes().unwrap();
        let mut receipt_gas = 0;
        let mut block_gas = 0;
        for nonce in 0..2 {
            // Creating a recipient charges state gas, separating receipt and block gas totals.
            let tx = TransactionSigned::Legacy(
                TxLegacy {
                    chain_id: Some(1),
                    nonce,
                    gas_limit: 1_000_000,
                    gas_price: 1,
                    to: TxKind::Call(Address::with_last_byte(0x20 + nonce as u8)),
                    value: U256::from(1),
                    ..Default::default()
                }
                .into_signed(Signature::test_signature()),
            );
            let gas = executor.execute_transaction(Recovered::new_unchecked(tx, sender)).unwrap();
            let segment_block_gas = gas.regular_gas_used().max(gas.state_gas_used());
            assert_ne!(gas.tx_gas_used(), segment_block_gas);
            receipt_gas += gas.tx_gas_used();
            block_gas += segment_block_gas;
            let receipt = executor.receipts().last().unwrap();
            assert!(receipt.success);
            assert_eq!(receipt.cumulative_gas_used, receipt_gas);
        }
        let (output, _) = executor.finish_with_block_access_list().unwrap();
        assert_eq!(output.result.gas_used, block_gas);
    }

    #[test]
    fn big_block_segment_switch_installs_segment_blob_cap() {
        let sender = Address::with_last_byte(0x10);
        let mut database = TestDatabase::default();
        database.accounts.insert(
            sender,
            AccountInfo::default().with_balance(U256::from(1_000_000_000_000_000_000u64)),
        );
        insert_request_contracts(&mut database);
        let segments = [9, 6]
            .into_iter()
            .enumerate()
            .map(|(index, max_blobs_per_tx)| {
                let mut segment = segment(index, index as u64 + 1, B256::ZERO);
                segment.ctx.parent_beacon_block_root = Some(B256::ZERO);
                segment.evm_env = EthEvmEnv::new(
                    SpecId::PRAGUE,
                    BlockEnv::<BaseEvmTypes> {
                        number: U256::from(index + 1),
                        gas_limit: U256::from(30_000_000),
                        ..Default::default()
                    },
                    1,
                );
                segment.evm_env.version.max_blobs_per_tx = max_blobs_per_tx;
                segment
            })
            .collect();
        let plan = EthBigBlockPlan::new(segments, Vec::new(), 2);
        let factory = super::super::factory::EthBigBlockExecutorFactory::new(
            super::super::factory::EthBlockExecutorFactory::new(Arc::new(
                ChainSpecBuilder::mainnet().prague_activated().build(),
            )),
        );
        let evm = factory.evm_with_env(Db::new(database), plan.segments[0].evm_env.clone());
        let mut executor = factory.create_executor(evm, plan);
        executor.apply_pre_execution_changes().unwrap();

        let blob_tx = |nonce| {
            let tx = TransactionSigned::Eip4844(
                TxEip4844 {
                    chain_id: 1,
                    nonce,
                    gas_limit: 100_000,
                    max_fee_per_gas: 1,
                    max_priority_fee_per_gas: 1,
                    to: Address::with_last_byte(0x20),
                    max_fee_per_blob_gas: 1,
                    blob_versioned_hashes: vec![B256::repeat_byte(1); 7],
                    ..Default::default()
                }
                .into_signed(Signature::test_signature()),
            );
            Recovered::new_unchecked(tx, sender)
        };
        executor.execute_transaction(blob_tx(0)).expect("seven blobs fit the first segment cap");
        let err = executor.execute_transaction(blob_tx(1)).unwrap_err();
        assert!(err.to_string().contains("too many blobs: have 7, max 6"), "{err}");
    }
}
