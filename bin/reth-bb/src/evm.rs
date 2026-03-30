//! Big-block EVM wrapper and executor.
//!
//! Provides [`BbEvm`], [`BbEvmFactory`], [`BbBlockExecutor`], and
//! [`BbBlockExecutorFactory`] which handle segment boundaries within
//! big-block payloads.
//!
//! # Architecture
//!
//! Boundary handling lives at the **executor** level because the [`EvmFactory`]
//! GAT requires the EVM to impl [`Evm`] for all `DB: Database`, while
//! boundary operations need `DB: StateDB`. The executor layer
//! ([`BlockExecutorFactory::create_executor`]) provides `DB: StateDB`.
//!
//! - [`BbEvm`] is a thin delegating wrapper around [`EthEvm`], carrying the plan but performing no
//!   boundary logic.
//! - [`BbBlockExecutor`] wraps [`EthBlockExecutor`] and intercepts `execute_transaction` to apply
//!   segment-boundary changes.

use crate::evm_config::BigBlockSegment;
use alloy_eips::eip7685::Requests;
use alloy_evm::{
    block::{
        BlockExecutionError, BlockExecutionResult, BlockExecutor, BlockExecutorFactory,
        BlockExecutorFor, ExecutableTx, OnStateHook, StateChangeSource, StateDB,
    },
    eth::{EthBlockExecutionCtx, EthBlockExecutor, EthEvmContext, EthTxResult},
    precompiles::PrecompilesMap,
    Database, EthEvm, EthEvmFactory, Evm, EvmEnv, EvmFactory, FromRecoveredTx, FromTxWithEncoded,
};
use alloy_primitives::{Address, Bytes, B256};
use reth_ethereum_primitives::{Receipt, TransactionSigned};
use reth_evm_ethereum::RethReceiptBuilder;
use revm::{
    context::{BlockEnv, TxEnv},
    context_interface::result::{EVMError, HaltReason, ResultAndState},
    handler::PrecompileProvider,
    inspector::NoOpInspector,
    interpreter::InterpreterResult,
    primitives::hardfork::SpecId,
    Inspector,
};
use std::sync::{Arc, Mutex};
use tracing::{debug, trace};

// ---------------------------------------------------------------------------
// BbEvmPlan — runtime segment tracking state
// ---------------------------------------------------------------------------

/// Runtime state for segment boundary tracking.
pub(crate) struct BbEvmPlan {
    /// The segment boundaries and environments.
    pub(crate) segments: Vec<BigBlockSegment>,
    /// Index of the next segment to switch to (starts at 1).
    pub(crate) next_segment: usize,
    /// Number of user transactions executed so far.
    pub(crate) tx_counter: usize,
    /// Block hashes to seed for inter-segment BLOCKHASH resolution.
    /// Includes both prior block hashes and inter-segment hashes.
    pub(crate) block_hashes_to_seed: Vec<(u64, B256)>,
}

impl BbEvmPlan {
    /// Creates a new `BbEvmPlan` from segments and hardfork flags.
    pub(crate) fn new(segments: Vec<BigBlockSegment>) -> Self {
        // Pre-compute all inter-segment block hashes.
        let mut block_hashes_to_seed = Vec::new();
        for seg in segments.iter().skip(1) {
            let finished_block_number = seg.evm_env.block_env.number.saturating_to::<u64>() - 1;
            let finished_block_hash = seg.ctx.parent_hash;
            block_hashes_to_seed.push((finished_block_number, finished_block_hash));
        }

        Self { segments, next_segment: 1, tx_counter: 0, block_hashes_to_seed }
    }
}

impl std::fmt::Debug for BbEvmPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BbEvmPlan")
            .field("segments", &self.segments)
            .field("next_segment", &self.next_segment)
            .field("tx_counter", &self.tx_counter)
            .field("block_hashes_to_seed", &self.block_hashes_to_seed)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// BbEvm — thin delegating EVM wrapper
// ---------------------------------------------------------------------------

/// An EVM wrapper that carries a [`BbEvmPlan`] for big-block execution.
///
/// All [`Evm`] methods delegate directly to the inner [`EthEvm`].
/// Boundary handling is performed by [`BbBlockExecutor`] at the executor layer.
#[allow(missing_debug_implementations)]
pub struct BbEvm<DB: Database, I, P = PrecompilesMap> {
    inner: EthEvm<DB, I, P>,
    plan: Option<BbEvmPlan>,
}

impl<DB: Database, I, P> BbEvm<DB, I, P> {
    /// Returns a reference to the plan, if any.
    pub(crate) const fn plan(&self) -> Option<&BbEvmPlan> {
        self.plan.as_ref()
    }

    /// Takes the plan out of this EVM, leaving `None` in its place.
    pub(crate) const fn take_plan(&mut self) -> Option<BbEvmPlan> {
        self.plan.take()
    }

    /// Provides a mutable reference to the inner EVM context.
    pub(crate) const fn ctx_mut(&mut self) -> &mut EthEvmContext<DB> {
        self.inner.ctx_mut()
    }
}

impl<DB, I, P> Evm for BbEvm<DB, I, P>
where
    DB: Database,
    I: Inspector<EthEvmContext<DB>>,
    P: PrecompileProvider<EthEvmContext<DB>, Output = InterpreterResult>,
{
    type DB = DB;
    type Tx = TxEnv;
    type Error = EVMError<DB::Error>;
    type HaltReason = HaltReason;
    type Spec = SpecId;
    type BlockEnv = BlockEnv;
    type Precompiles = P;
    type Inspector = I;

    fn block(&self) -> &BlockEnv {
        self.inner.block()
    }

    fn chain_id(&self) -> u64 {
        self.inner.chain_id()
    }

    fn transact_raw(
        &mut self,
        tx: Self::Tx,
    ) -> Result<ResultAndState<Self::HaltReason>, Self::Error> {
        self.inner.transact_raw(tx)
    }

    fn transact_system_call(
        &mut self,
        caller: Address,
        contract: Address,
        data: Bytes,
    ) -> Result<ResultAndState<Self::HaltReason>, Self::Error> {
        self.inner.transact_system_call(caller, contract, data)
    }

    fn finish(self) -> (Self::DB, EvmEnv<Self::Spec, Self::BlockEnv>) {
        self.inner.finish()
    }

    fn set_inspector_enabled(&mut self, enabled: bool) {
        self.inner.set_inspector_enabled(enabled);
    }

    fn components(&self) -> (&Self::DB, &Self::Inspector, &Self::Precompiles) {
        self.inner.components()
    }

    fn components_mut(&mut self) -> (&mut Self::DB, &mut Self::Inspector, &mut Self::Precompiles) {
        self.inner.components_mut()
    }
}

// ---------------------------------------------------------------------------
// BbEvmFactory
// ---------------------------------------------------------------------------

/// Factory producing [`BbEvm`] instances.
#[derive(Debug, Clone)]
pub struct BbEvmFactory {
    inner: EthEvmFactory,
    /// Staged plan to inject into the next created `BbEvm`.
    pub(crate) staged_plan: Arc<Mutex<Option<BbEvmPlan>>>,
}

impl BbEvmFactory {
    pub fn new(inner: EthEvmFactory) -> Self {
        Self { inner, staged_plan: Arc::new(Mutex::new(None)) }
    }

    pub(crate) fn stage_plan(&self, plan: BbEvmPlan) {
        *self.staged_plan.lock().unwrap() = Some(plan);
    }

    fn take_plan(&self) -> Option<BbEvmPlan> {
        self.staged_plan.lock().unwrap().take()
    }
}

impl EvmFactory for BbEvmFactory {
    type Evm<DB: Database, I: Inspector<EthEvmContext<DB>>> = BbEvm<DB, I, PrecompilesMap>;
    type Context<DB: Database> = EthEvmContext<DB>;
    type Tx = TxEnv;
    type Error<DBError: core::error::Error + Send + Sync + 'static> = EVMError<DBError>;
    type HaltReason = HaltReason;
    type Spec = SpecId;
    type BlockEnv = BlockEnv;
    type Precompiles = PrecompilesMap;

    fn create_evm<DB: Database>(&self, db: DB, evm_env: EvmEnv) -> Self::Evm<DB, NoOpInspector> {
        let inner = self.inner.create_evm(db, evm_env);
        BbEvm { inner, plan: self.take_plan() }
    }

    fn create_evm_with_inspector<DB: Database, I: Inspector<EthEvmContext<DB>>>(
        &self,
        db: DB,
        evm_env: EvmEnv,
        inspector: I,
    ) -> Self::Evm<DB, I> {
        let inner = self.inner.create_evm_with_inspector(db, evm_env, inspector);
        BbEvm { inner, plan: self.take_plan() }
    }
}

// ---------------------------------------------------------------------------
// BbBlockExecutor — handles segment boundaries
// ---------------------------------------------------------------------------

/// Block executor that wraps [`EthBlockExecutor`] and handles segment-boundary
/// changes for big-block execution.
///
/// At segment boundaries, the inner executor is finished (applying its
/// end-of-block logic: post-execution system calls, withdrawal balance
/// increments) and a new one is constructed for the next segment (applying
/// its start-of-block logic: EIP-2935/EIP-4788 system calls).
///
/// Gas counters reset at each boundary so that each segment's real gas limit
/// is used (preserving correct GASLIMIT opcode behavior). Accumulated offsets
/// are applied to receipts and totals in `finish()`.
pub(crate) struct BbBlockExecutor<'a, DB, I, P, Spec>
where
    DB: Database,
{
    /// The inner executor. `None` transiently during `apply_segment_boundary`.
    inner: Option<EthBlockExecutor<'a, BbEvm<DB, I, P>, Spec, RethReceiptBuilder>>,
    plan: Option<BbEvmPlan>,
    /// Requests accumulated from segments that have been finished at
    /// boundaries. Merged into the final result in `finish()`.
    accumulated_requests: Requests,
    /// Cumulative gas used by all segments that have been finished at
    /// boundaries. Added to receipts and the final gas total in `finish()`.
    gas_used_offset: u64,
    /// Cumulative blob gas used by all segments that have been finished at
    /// boundaries.
    blob_gas_used_offset: u64,
    /// Shared state hook that survives inner executor finish/reconstruct
    /// cycles at segment boundaries. Each inner executor receives a
    /// forwarding hook that delegates to this shared instance.
    shared_hook: Arc<Mutex<Option<Box<dyn OnStateHook>>>>,
}

impl<'a, DB, I, P, Spec> BbBlockExecutor<'a, DB, I, P, Spec>
where
    DB: StateDB,
    I: Inspector<EthEvmContext<DB>>,
    P: PrecompileProvider<EthEvmContext<DB>, Output = InterpreterResult>,
    Spec: alloy_evm::eth::spec::EthExecutorSpec + Clone,
    BbEvm<DB, I, P>: Evm<
        DB = DB,
        Tx = TxEnv,
        HaltReason = HaltReason,
        Error = EVMError<DB::Error>,
        Spec = SpecId,
        BlockEnv = BlockEnv,
    >,
    TxEnv: FromRecoveredTx<TransactionSigned> + FromTxWithEncoded<TransactionSigned>,
{
    pub(crate) fn new(
        mut evm: BbEvm<DB, I, P>,
        ctx: EthBlockExecutionCtx<'a>,
        spec: Spec,
        receipt_builder: RethReceiptBuilder,
    ) -> Self {
        let plan = evm.take_plan();
        let inner = EthBlockExecutor::new(evm, ctx, spec, receipt_builder);
        Self {
            inner: Some(inner),
            plan,
            accumulated_requests: Requests::default(),
            gas_used_offset: 0,
            blob_gas_used_offset: 0,
            shared_hook: Arc::new(Mutex::new(None)),
        }
    }

    /// Creates a forwarding `OnStateHook` that delegates to the shared hook.
    fn forwarding_hook(&self) -> Option<Box<dyn OnStateHook>> {
        let shared = self.shared_hook.clone();
        Some(Box::new(move |source: StateChangeSource, state: &revm::state::EvmState| {
            if let Some(hook) = shared.lock().unwrap().as_mut() {
                hook.on_state(source, state);
            }
        }))
    }

    const fn inner(&self) -> &EthBlockExecutor<'a, BbEvm<DB, I, P>, Spec, RethReceiptBuilder> {
        self.inner.as_ref().expect("inner executor must exist")
    }

    const fn inner_mut(
        &mut self,
    ) -> &mut EthBlockExecutor<'a, BbEvm<DB, I, P>, Spec, RethReceiptBuilder> {
        self.inner.as_mut().expect("inner executor must exist")
    }

    fn maybe_apply_boundary(&mut self) -> Result<(), BlockExecutionError> {
        loop {
            let plan = match &self.plan {
                Some(p) => p,
                None => return Ok(()),
            };

            if plan.next_segment >= plan.segments.len() ||
                plan.tx_counter != plan.segments[plan.next_segment].start_tx
            {
                return Ok(());
            }

            self.apply_segment_boundary()?;
        }
    }

    fn apply_segment_boundary(&mut self) -> Result<(), BlockExecutionError> {
        let plan = self.plan.as_mut().expect("plan must exist");
        let seg_idx = plan.next_segment;
        let prev_seg_idx = seg_idx - 1;

        debug!(
            target: "engine::bb::evm",
            seg_idx,
            tx_counter = plan.tx_counter,
            "Applying segment boundary"
        );

        // Swap the inner executor's ctx to the finished segment's values so
        // that finish() applies the correct withdrawals and post-execution
        // system calls for that segment.
        let prev_segment = &plan.segments[prev_seg_idx];
        let prev_ctx = EthBlockExecutionCtx {
            parent_hash: prev_segment.ctx.parent_hash,
            parent_beacon_block_root: prev_segment.ctx.parent_beacon_block_root,
            ommers: prev_segment.ctx.ommers,
            withdrawals: prev_segment.ctx.withdrawals.clone(),
            extra_data: prev_segment.ctx.extra_data.clone(),
            tx_count_hint: prev_segment.ctx.tx_count_hint,
        };

        // Clone the next segment's data before we consume inner.
        let new_segment = &plan.segments[seg_idx];
        let new_block_env = new_segment.evm_env.block_env.clone();
        let mut new_cfg_env = new_segment.evm_env.cfg_env.clone();
        new_cfg_env.disable_base_fee = true;
        let new_ctx = EthBlockExecutionCtx {
            parent_hash: new_segment.ctx.parent_hash,
            parent_beacon_block_root: new_segment.ctx.parent_beacon_block_root,
            ommers: new_segment.ctx.ommers,
            withdrawals: new_segment.ctx.withdrawals.clone(),
            extra_data: new_segment.ctx.extra_data.clone(),
            tx_count_hint: new_segment.ctx.tx_count_hint,
        };

        plan.next_segment += 1;

        // Finish the inner executor for the completed segment. This applies
        // post-execution system calls (EIP-7002/7251) and withdrawal balance
        // increments via EthBlockExecutor::finish().
        let mut inner = self.inner.take().expect("inner executor must exist");
        inner.ctx = prev_ctx;
        let spec = inner.spec.clone();
        let receipt_builder = inner.receipt_builder;
        let (mut evm, result) = inner.finish()?;

        // Receipts already have globally-correct cumulative_gas_used (fixed
        // up in commit_transaction). Update the offset with this segment's
        // gas so that subsequent segments' receipts are adjusted correctly.
        self.gas_used_offset += result.gas_used;
        self.blob_gas_used_offset += result.blob_gas_used;
        self.accumulated_requests.extend(result.requests);

        trace!(
            target: "engine::bb::evm",
            "Finished segment {prev_seg_idx}, receipts={}, gas_used={}",
            result.receipts.len(),
            result.gas_used,
        );

        // Swap EVM env to the next segment's values (using real gas_limit).
        let ctx = evm.ctx_mut();
        ctx.block = new_block_env;
        ctx.cfg = new_cfg_env;

        // Build a new inner executor for the next segment. gas_used starts
        // at 0 so the per-transaction gas check uses this segment's real
        // gas_limit correctly.
        let mut new_inner = EthBlockExecutor::new(evm, new_ctx, spec, receipt_builder);

        // Carry forward receipts from prior segments.
        new_inner.receipts = result.receipts;

        // Re-install the forwarding state hook so the parallel state root
        // task continues to receive state changes.
        if self.shared_hook.lock().unwrap().is_some() {
            new_inner.set_state_hook(self.forwarding_hook());
        }

        // Apply pre-execution changes for the new segment (EIP-2935, EIP-4788).
        new_inner.apply_pre_execution_changes()?;

        trace!(target: "engine::bb::evm", "Started segment {seg_idx}");

        self.inner = Some(new_inner);
        Ok(())
    }
}

impl<'a, DB, I, P, Spec> BlockExecutor for BbBlockExecutor<'a, DB, I, P, Spec>
where
    DB: StateDB,
    I: Inspector<EthEvmContext<DB>>,
    P: PrecompileProvider<EthEvmContext<DB>, Output = InterpreterResult>,
    Spec: alloy_evm::eth::spec::EthExecutorSpec + Clone,
    BbEvm<DB, I, P>: Evm<
        DB = DB,
        Tx = TxEnv,
        HaltReason = HaltReason,
        Error = EVMError<DB::Error>,
        Spec = SpecId,
        BlockEnv = BlockEnv,
    >,
    TxEnv: FromRecoveredTx<TransactionSigned> + FromTxWithEncoded<TransactionSigned>,
{
    type Transaction = TransactionSigned;
    type Receipt = Receipt;
    type Evm = BbEvm<DB, I, P>;
    type Result = EthTxResult<HaltReason, alloy_consensus::TxType>;

    fn apply_pre_execution_changes(&mut self) -> Result<(), BlockExecutionError> {
        // Swap the EVM's block_env and executor ctx to the first segment's
        // values so that the initial EIP-2935/EIP-4788 system calls use the
        // correct block number and parent hash. Without this, the outer big
        // block header's block_number (which is synthetic) would be used,
        // writing to wrong EIP-2935 slots and corrupting state.
        if let Some(seg0) = self.plan.as_ref().map(|p| &p.segments[0]) {
            let block_env = seg0.evm_env.block_env.clone();
            let mut cfg_env = seg0.evm_env.cfg_env.clone();
            cfg_env.disable_base_fee = true;
            let seg0_ctx = EthBlockExecutionCtx {
                parent_hash: seg0.ctx.parent_hash,
                parent_beacon_block_root: seg0.ctx.parent_beacon_block_root,
                ommers: seg0.ctx.ommers,
                withdrawals: seg0.ctx.withdrawals.clone(),
                extra_data: seg0.ctx.extra_data.clone(),
                tx_count_hint: seg0.ctx.tx_count_hint,
            };

            let inner = self.inner_mut();
            let evm_ctx = inner.evm.ctx_mut();
            evm_ctx.block = block_env;
            evm_ctx.cfg = cfg_env;
            inner.ctx = seg0_ctx;
        }

        self.inner_mut().apply_pre_execution_changes()
    }

    fn execute_transaction_without_commit(
        &mut self,
        tx: impl ExecutableTx<Self>,
    ) -> Result<Self::Result, BlockExecutionError> {
        self.maybe_apply_boundary()?;
        self.inner_mut().execute_transaction_without_commit(tx)
    }

    fn commit_transaction(&mut self, output: Self::Result) -> Result<u64, BlockExecutionError> {
        let gas_used = self.inner_mut().commit_transaction(output)?;

        // Fix up cumulative_gas_used on the just-committed receipt so that
        // the receipt root task (which reads receipts incrementally) sees
        // globally-correct values across all segments.
        let offset = self.gas_used_offset;
        if offset > 0 &&
            let Some(receipt) = self.inner_mut().receipts.last_mut()
        {
            receipt.cumulative_gas_used += offset;
        }

        if let Some(plan) = &mut self.plan {
            plan.tx_counter += 1;
        }
        Ok(gas_used)
    }

    fn finish(
        mut self,
    ) -> Result<(Self::Evm, BlockExecutionResult<Self::Receipt>), BlockExecutionError> {
        // Swap the inner executor's ctx to the last segment's ctx so that
        // EthBlockExecutor::finish() applies the correct withdrawal balance
        // increments and post-execution system calls.
        if let Some(last_seg) = self.plan.as_ref().map(|p| p.segments.last().unwrap()) {
            let last_ctx = EthBlockExecutionCtx {
                parent_hash: last_seg.ctx.parent_hash,
                parent_beacon_block_root: last_seg.ctx.parent_beacon_block_root,
                ommers: last_seg.ctx.ommers,
                withdrawals: last_seg.ctx.withdrawals.clone(),
                extra_data: last_seg.ctx.extra_data.clone(),
                tx_count_hint: last_seg.ctx.tx_count_hint,
            };
            self.inner_mut().ctx = last_ctx;
        }
        let inner = self.inner.take().expect("inner executor must exist");
        let (evm, mut result) = inner.finish()?;

        // Receipts already have globally-correct cumulative_gas_used (fixed
        // up in commit_transaction). Add the offset to the totals so they
        // reflect gas across all segments.
        result.gas_used += self.gas_used_offset;
        result.blob_gas_used += self.blob_gas_used_offset;

        // Merge requests accumulated from earlier segment boundaries into
        // the final result.
        if !self.accumulated_requests.is_empty() {
            let mut merged = self.accumulated_requests;
            merged.extend(result.requests);
            result.requests = merged;
        }

        Ok((evm, result))
    }

    fn set_state_hook(&mut self, hook: Option<Box<dyn OnStateHook>>) {
        if self.plan.is_some() {
            // Store the real hook in the shared slot and give the inner
            // executor a lightweight forwarder. This way the hook survives
            // inner executor finish/reconstruct cycles at segment boundaries.
            *self.shared_hook.lock().unwrap() = hook;
            let fwd = self.forwarding_hook();
            self.inner_mut().set_state_hook(fwd);
        } else {
            self.inner_mut().set_state_hook(hook);
        }
    }

    fn evm_mut(&mut self) -> &mut Self::Evm {
        self.inner_mut().evm_mut()
    }

    fn evm(&self) -> &Self::Evm {
        self.inner().evm()
    }

    fn receipts(&self) -> &[Self::Receipt] {
        self.inner().receipts()
    }
}

// ---------------------------------------------------------------------------
// BbBlockExecutorFactory
// ---------------------------------------------------------------------------

/// Block executor factory that uses [`BbEvmFactory`] to produce [`BbEvm`]
/// instances and [`BbBlockExecutor`] for boundary-aware execution.
#[derive(Debug, Clone)]
pub struct BbBlockExecutorFactory<Spec> {
    receipt_builder: RethReceiptBuilder,
    spec: Spec,
    evm_factory: BbEvmFactory,
}

impl<Spec> BbBlockExecutorFactory<Spec> {
    pub const fn new(
        receipt_builder: RethReceiptBuilder,
        spec: Spec,
        evm_factory: BbEvmFactory,
    ) -> Self {
        Self { receipt_builder, spec, evm_factory }
    }

    pub const fn evm_factory(&self) -> &BbEvmFactory {
        &self.evm_factory
    }

    pub const fn spec(&self) -> &Spec {
        &self.spec
    }

    pub const fn receipt_builder(&self) -> &RethReceiptBuilder {
        &self.receipt_builder
    }
}

impl<Spec> BlockExecutorFactory for BbBlockExecutorFactory<Spec>
where
    Spec: alloy_evm::eth::spec::EthExecutorSpec + 'static,
    TxEnv: FromRecoveredTx<TransactionSigned> + FromTxWithEncoded<TransactionSigned>,
{
    type EvmFactory = BbEvmFactory;
    type ExecutionCtx<'a> = EthBlockExecutionCtx<'a>;
    type Transaction = TransactionSigned;
    type Receipt = Receipt;

    fn evm_factory(&self) -> &Self::EvmFactory {
        &self.evm_factory
    }

    fn create_executor<'a, DB, I>(
        &'a self,
        evm: BbEvm<DB, I, PrecompilesMap>,
        ctx: EthBlockExecutionCtx<'a>,
    ) -> impl BlockExecutorFor<'a, Self, DB, I>
    where
        DB: StateDB + 'a,
        I: Inspector<EthEvmContext<DB>> + 'a,
    {
        BbBlockExecutor::new(evm, ctx, &self.spec, self.receipt_builder)
    }
}
