//! Big-block executor.
//!
//! Provides [`BbBlockExecutor`] and [`BbBlockExecutorFactory`] which handle
//! segment boundaries within big-block payloads.
//!
//! [`BbBlockExecutor`] wraps [`EthBlockExecutor`] and intercepts
//! `execute_transaction` to apply segment-boundary changes.

use crate::evm_config::BigBlockSegment;
use alloy_eips::eip7685::Requests;
use alloy_evm::{
    block::{
        BlockExecutionError, BlockExecutionResult, BlockExecutor, BlockExecutorFactory,
        BlockExecutorFor, ExecutableTx, GasOutput, OnStateHook, StateChangeSource, StateDB,
    },
    eth::{EthBlockExecutionCtx, EthBlockExecutor, EthEvmContext, EthTxResult},
    precompiles::PrecompilesMap,
    Database, EthEvm, EthEvmFactory, Evm, FromRecoveredTx, FromTxWithEncoded,
};
use alloy_primitives::B256;
use reth_ethereum_primitives::{Receipt, TransactionSigned};
use reth_evm_ethereum::RethReceiptBuilder;
use revm::{
    context::{BlockEnv, TxEnv},
    context_interface::result::{EVMError, HaltReason},
    handler::PrecompileProvider,
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

    /// Returns the 256 block hashes relevant to a segment with the given block
    /// number. BLOCKHASH can look back 256 blocks, so we select entries in
    /// `[block_number - 256, block_number)`.
    pub(crate) fn hashes_for_block(&self, block_number: u64) -> Vec<(u64, B256)> {
        let min = block_number.saturating_sub(256);
        self.block_hashes_to_seed
            .iter()
            .copied()
            .filter(|(n, _)| *n >= min && *n < block_number)
            .collect()
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
// BbBlockExecutor — handles segment boundaries
// ---------------------------------------------------------------------------

/// Function pointer that seeds block hashes into the DB's block hash cache.
///
/// Injected from `ConfigureEvm::create_executor` where the concrete `State<DB>`
/// type is known, allowing `BbBlockExecutor` to reseed the ring buffer at
/// segment boundaries without requiring additional trait bounds on `DB`.
pub(crate) type BlockHashSeeder<DB> = fn(&mut DB, &[(u64, B256)]);

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
    inner: Option<EthBlockExecutor<'a, EthEvm<DB, I, P>, Spec, RethReceiptBuilder>>,
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
    /// Callback to reseed block hashes into the DB's cache at segment
    /// boundaries. See [`BlockHashSeeder`].
    block_hash_seeder: Option<BlockHashSeeder<DB>>,
}

impl<'a, DB, I, P, Spec> BbBlockExecutor<'a, DB, I, P, Spec>
where
    DB: StateDB,
    I: Inspector<EthEvmContext<DB>>,
    P: PrecompileProvider<EthEvmContext<DB>, Output = InterpreterResult>,
    Spec: alloy_evm::eth::spec::EthExecutorSpec + Clone,
    EthEvm<DB, I, P>: Evm<
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
        evm: EthEvm<DB, I, P>,
        ctx: EthBlockExecutionCtx<'a>,
        spec: Spec,
        receipt_builder: RethReceiptBuilder,
        plan: Option<BbEvmPlan>,
        block_hash_seeder: Option<BlockHashSeeder<DB>>,
    ) -> Self {
        let inner = EthBlockExecutor::new(evm, ctx, spec, receipt_builder);
        Self {
            inner: Some(inner),
            plan,
            accumulated_requests: Requests::default(),
            gas_used_offset: 0,
            blob_gas_used_offset: 0,
            shared_hook: Arc::new(Mutex::new(None)),
            block_hash_seeder,
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

    const fn inner(&self) -> &EthBlockExecutor<'a, EthEvm<DB, I, P>, Spec, RethReceiptBuilder> {
        self.inner.as_ref().expect("inner executor must exist")
    }

    const fn inner_mut(
        &mut self,
    ) -> &mut EthBlockExecutor<'a, EthEvm<DB, I, P>, Spec, RethReceiptBuilder> {
        self.inner.as_mut().expect("inner executor must exist")
    }

    fn reseed_block_hashes_for(&mut self, block_number: u64) {
        let Some(seeder) = self.block_hash_seeder else { return };
        let hashes = match &self.plan {
            Some(plan) => plan.hashes_for_block(block_number),
            None => return,
        };
        seeder(self.inner_mut().evm_mut().db_mut(), &hashes);
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

        let last_receipt_cumulative =
            result.receipts.last().map(|r| r.cumulative_gas_used).unwrap_or(0);
        let seg_block_number = prev_segment.evm_env.block_env.number.saturating_to::<u64>();
        debug!(
            target: "engine::bb::evm",
            prev_seg_idx,
            seg_block_number,
            segment_gas_used = result.gas_used,
            gas_used_offset = self.gas_used_offset,
            last_receipt_cumulative,
            receipt_count = result.receipts.len(),
            "Finished segment"
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

        self.inner = Some(new_inner);

        // Reseed the block hash cache for the new segment's 256-block window
        // before applying pre-execution changes (which may use BLOCKHASH).
        let new_block_number = self.plan.as_ref().unwrap().segments[seg_idx]
            .evm_env
            .block_env
            .number
            .saturating_to::<u64>();
        self.reseed_block_hashes_for(new_block_number);

        // Apply pre-execution changes for the new segment (EIP-2935, EIP-4788).
        self.inner_mut().apply_pre_execution_changes()?;

        trace!(target: "engine::bb::evm", "Started segment {seg_idx}");

        Ok(())
    }
}

impl<'a, DB, I, P, Spec> BlockExecutor for BbBlockExecutor<'a, DB, I, P, Spec>
where
    DB: StateDB,
    I: Inspector<EthEvmContext<DB>>,
    P: PrecompileProvider<EthEvmContext<DB>, Output = InterpreterResult>,
    Spec: alloy_evm::eth::spec::EthExecutorSpec + Clone,
    EthEvm<DB, I, P>: Evm<
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
    type Evm = EthEvm<DB, I, P>;
    type Result = EthTxResult<HaltReason, alloy_consensus::TxType>;

    fn apply_pre_execution_changes(&mut self) -> Result<(), BlockExecutionError> {
        // Swap the EVM's block_env and executor ctx to the first segment's
        // values so that the initial EIP-2935/EIP-4788 system calls use the
        // correct block number and parent hash. Without this, the outer big
        // block header's block_number (which is synthetic) would be used,
        // writing to wrong EIP-2935 slots and corrupting state.
        if let Some(seg0) = self.plan.as_ref().map(|p| &p.segments[0]) {
            let block_env = seg0.evm_env.block_env.clone();
            let block_number = block_env.number.saturating_to::<u64>();
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

            self.reseed_block_hashes_for(block_number);
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

    fn commit_transaction(
        &mut self,
        output: Self::Result,
    ) -> Result<GasOutput, BlockExecutionError> {
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
        let last_segment_gas = result.gas_used;
        result.gas_used += self.gas_used_offset;
        result.blob_gas_used += self.blob_gas_used_offset;

        let last_receipt_cumulative =
            result.receipts.last().map(|r| r.cumulative_gas_used).unwrap_or(0);
        debug!(
            target: "engine::bb::evm",
            last_segment_gas,
            gas_used_offset = self.gas_used_offset,
            total_gas_used = result.gas_used,
            last_receipt_cumulative,
            receipt_count = result.receipts.len(),
            "Finished final segment"
        );

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

/// Block executor factory that produces [`BbBlockExecutor`] for
/// boundary-aware big-block execution.
#[derive(Debug, Clone)]
pub struct BbBlockExecutorFactory<Spec> {
    receipt_builder: RethReceiptBuilder,
    spec: Spec,
    evm_factory: EthEvmFactory,
    /// Staged plan consumed by the next [`BbBlockExecutor`].
    pub(crate) staged_plan: Arc<Mutex<Option<BbEvmPlan>>>,
}

impl<Spec> BbBlockExecutorFactory<Spec> {
    pub fn new(
        receipt_builder: RethReceiptBuilder,
        spec: Spec,
        evm_factory: EthEvmFactory,
    ) -> Self {
        Self { receipt_builder, spec, evm_factory, staged_plan: Arc::new(Mutex::new(None)) }
    }

    pub const fn evm_factory(&self) -> &EthEvmFactory {
        &self.evm_factory
    }

    pub const fn spec(&self) -> &Spec {
        &self.spec
    }

    pub const fn receipt_builder(&self) -> &RethReceiptBuilder {
        &self.receipt_builder
    }

    pub(crate) fn stage_plan(&self, plan: BbEvmPlan) {
        *self.staged_plan.lock().unwrap() = Some(plan);
    }

    fn take_plan(&self) -> Option<BbEvmPlan> {
        self.staged_plan.lock().unwrap().take()
    }

    pub(crate) fn create_executor_with_seeder<'a, DB, I>(
        &'a self,
        evm: EthEvm<DB, I, PrecompilesMap>,
        ctx: EthBlockExecutionCtx<'a>,
        block_hash_seeder: Option<BlockHashSeeder<DB>>,
    ) -> BbBlockExecutor<'a, DB, I, PrecompilesMap, &'a Spec>
    where
        Spec: alloy_evm::eth::spec::EthExecutorSpec,
        DB: StateDB + 'a,
        I: Inspector<EthEvmContext<DB>> + 'a,
    {
        let plan = self.take_plan();
        BbBlockExecutor::new(evm, ctx, &self.spec, self.receipt_builder, plan, block_hash_seeder)
    }
}

impl<Spec> BlockExecutorFactory for BbBlockExecutorFactory<Spec>
where
    Spec: alloy_evm::eth::spec::EthExecutorSpec + 'static,
    TxEnv: FromRecoveredTx<TransactionSigned> + FromTxWithEncoded<TransactionSigned>,
{
    type EvmFactory = EthEvmFactory;
    type ExecutionCtx<'a> = EthBlockExecutionCtx<'a>;
    type Transaction = TransactionSigned;
    type Receipt = Receipt;

    fn evm_factory(&self) -> &Self::EvmFactory {
        &self.evm_factory
    }

    fn create_executor<'a, DB, I>(
        &'a self,
        evm: EthEvm<DB, I, PrecompilesMap>,
        ctx: EthBlockExecutionCtx<'a>,
    ) -> impl BlockExecutorFor<'a, Self, DB, I>
    where
        DB: StateDB + 'a,
        I: Inspector<EthEvmContext<DB>> + 'a,
    {
        let plan = self.take_plan();
        BbBlockExecutor::new(evm, ctx, &self.spec, self.receipt_builder, plan, None)
    }
}
