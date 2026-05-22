//! Big-block executor.
//!
//! Provides [`BbBlockExecutor`] and [`BbBlockExecutorFactory`] which handle
//! segment boundaries within big-block payloads.
//!
//! [`BbBlockExecutor`] wraps [`EthBlockExecutor`] and intercepts
//! `execute_transaction` to apply segment-boundary changes.

use crate::evm_config::BigBlockSegment;
use alloy_consensus::TransactionEnvelope;
use alloy_eips::eip7685::Requests;
use alloy_evm::{
    block::{
        BlockExecutionError, BlockExecutionResult, BlockExecutor, BlockExecutorFactory,
        ExecutableTx, GasOutput, OnStateHook, StateChangeSource, StateDB,
    },
    eth::{EthBlockExecutionCtx, EthBlockExecutor, EthEvmContext, EthTxResult},
    precompiles::PrecompilesMap,
    Database, EthEvm, EthEvmFactory, Evm, EvmFactory, FromRecoveredTx, FromTxWithEncoded,
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
#[derive(Debug, Clone)]
pub struct BbEvmPlan<'a> {
    /// The segment boundaries and environments.
    pub(crate) segments: Vec<BigBlockSegment<'a>>,
    /// Index of the next segment to switch to (starts at 1).
    pub(crate) next_segment: usize,
    /// Transaction index where the next segment starts, or `usize::MAX` when exhausted.
    pub(crate) next_segment_start_tx: usize,
    /// Number of user transactions executed so far.
    pub(crate) tx_counter: usize,
    /// Block hashes to seed for inter-segment BLOCKHASH resolution.
    /// Includes both prior block hashes and inter-segment hashes.
    pub(crate) block_hashes_to_seed: Vec<(u64, B256)>,
}

impl<'a> BbEvmPlan<'a> {
    /// Creates a new `BbEvmPlan` from segments and hardfork flags.
    pub(crate) fn new(segments: Vec<BigBlockSegment<'a>>) -> Self {
        // Pre-compute all inter-segment block hashes.
        let mut block_hashes_to_seed = Vec::new();
        for seg in segments.iter().skip(1) {
            let finished_block_number = seg.evm_env.block_env.number.saturating_to::<u64>() - 1;
            let finished_block_hash = seg.ctx.parent_hash;
            block_hashes_to_seed.push((finished_block_number, finished_block_hash));
        }
        let next_segment_start_tx = segments.get(1).map_or(usize::MAX, |segment| segment.start_tx);

        Self {
            segments,
            next_segment: 1,
            next_segment_start_tx,
            tx_counter: 0,
            block_hashes_to_seed,
        }
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

    /// Returns the segment that contains the transaction at `tx_index`.
    pub(crate) fn segment_index_for_tx(&self, tx_index: usize) -> usize {
        self.segments.partition_point(|segment| segment.start_tx <= tx_index).saturating_sub(1)
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

/// Function pointer that reads the BAL index from the DB.
///
/// Injected from `ConfigureEvm::create_executor` where the concrete
/// `State<DB>` type is known. `BbBlockExecutor` calls this lazily on first
/// use to decide which segment to start at: `0` means canonical execution
/// from segment 0, `n > 0` means a BAL worker that runs only the n-th
/// transaction (in whichever segment contains it).
pub(crate) type BalIndexReader<DB> = fn(&DB) -> u64;

/// Function pointer that bumps the BAL index in the DB.
///
/// Injected from `ConfigureEvm::create_executor` like the other DB callbacks
/// so `BbBlockExecutor` can advance `bal_index` between sub-events of a
/// segment boundary (post-N's `finish()` and pre-N+1's
/// `apply_pre_execution_changes()`) without requiring additional trait bounds
/// on `DB`. The renumbering scheme places these on consecutive `bal_indexes` so
/// workers reading the BAL overlay see post-N's writes via the strict
/// less-than `BalWrites::get` semantic.
pub(crate) type BalIndexBumper<DB> = fn(&mut DB);

/// Function pointer that overwrites the BAL index in the DB.
///
/// Used in `BbBlockExecutor::initialize` to map a worker's incoming
/// `bal_index = i + 1` (the standard "tx i + 1" convention from
/// `execute_block_in_pool`) onto the renumbered space `i + 1 + 2k`, where
/// `k` is the segment index containing tx `i`. Renumbering reserves two
/// extra `bal_indexes` per segment boundary (one for each segment's
/// post-execution and one for the next segment's pre-execution), so workers'
/// strict less-than reads can see those boundary writes.
pub(crate) type BalIndexSetter<DB> = fn(&mut DB, u64);

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
#[expect(missing_debug_implementations)]
pub struct BbBlockExecutor<'a, DB, I, P, Spec>
where
    DB: Database,
{
    /// The inner executor. `None` transiently during `apply_segment_boundary`.
    inner: Option<EthBlockExecutor<'a, EthEvm<DB, I, P>, Spec, RethReceiptBuilder>>,
    plan: BbEvmPlan<'a>,
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
    /// Callback to read `bal_index` from the DB. See [`BalIndexReader`].
    bal_index_reader: Option<BalIndexReader<DB>>,
    /// Callback to bump `bal_index` on the DB. See [`BalIndexBumper`]. Used at
    /// segment boundaries to put post-N's writes and pre-N+1's writes on
    /// consecutive `bal_indexes`.
    bal_index_bumper: Option<BalIndexBumper<DB>>,
    /// Callback to set `bal_index` on the DB. See [`BalIndexSetter`]. Used in
    /// [`Self::initialize`] to renumber a worker's incoming `bal_index` into
    /// the boundary-padded space.
    bal_index_setter: Option<BalIndexSetter<DB>>,
    /// Whether the executor has selected its starting segment.
    initialized: bool,
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
    #[expect(clippy::too_many_arguments)]
    pub(crate) fn new(
        evm: EthEvm<DB, I, P>,
        plan: BbEvmPlan<'a>,
        spec: Spec,
        receipt_builder: RethReceiptBuilder,
        block_hash_seeder: Option<BlockHashSeeder<DB>>,
        bal_index_reader: Option<BalIndexReader<DB>>,
        bal_index_bumper: Option<BalIndexBumper<DB>>,
        bal_index_setter: Option<BalIndexSetter<DB>>,
    ) -> Self {
        let inner = EthBlockExecutor::new(evm, plan.segments[0].ctx.clone(), spec, receipt_builder);
        Self {
            inner: Some(inner),
            plan,
            accumulated_requests: Requests::default(),
            gas_used_offset: 0,
            blob_gas_used_offset: 0,
            shared_hook: Arc::new(Mutex::new(None)),
            block_hash_seeder,
            bal_index_reader,
            bal_index_bumper,
            bal_index_setter,
            initialized: false,
        }
    }

    /// Idempotent first-use init. Reads `bal_index` from the DB to pick the
    /// starting segment, swaps the inner executor's env/ctx to that segment,
    /// and reseeds the block hash cache for its window.
    ///
    /// `bal_index == 0` selects segment 0 — canonical execution that walks
    /// all segments via `maybe_apply_boundary`. `bal_index > 0` selects the
    /// segment containing the `(bal_index - 1)`-th transaction and drops the
    /// plan so segment boundaries don't fire — a BAL worker only runs one
    /// transaction in its segment.
    fn initialize(&mut self) -> Result<(), BlockExecutionError> {
        if self.initialized {
            return Ok(());
        }

        let segment_idx = if let Some(bal_index) = self
            .bal_index_reader
            .map(|reader| reader(self.inner().evm().db()))
            .filter(|bal_index| *bal_index > 0)
        {
            let segment_idx = self.plan.segment_index_for_tx((bal_index - 1) as usize);

            // Renumber the worker's bal_index from the raw "tx i + 1"
            // convention to "tx i + 1 + 2k" where k is the segment index.
            // This reserves two `bal_indexes` per crossed segment boundary
            // (one for post-N's `finish()`, one for pre-N+1's
            // `apply_pre_execution_changes`) so worker reads via
            // `BalWrites::get` see those writes via the strict less-than
            // semantic.
            if let Some(setter) = self.bal_index_setter {
                let renumbered = bal_index + 2 * segment_idx as u64;
                setter(self.inner_mut().evm_mut().db_mut(), renumbered);
            }

            segment_idx
        } else {
            if self.initialized {
                return Ok(());
            }

            self.initialized = true;
            0
        };
        let segment = &self.plan.segments[segment_idx];
        let block_env = segment.evm_env.block_env.clone();
        let block_number = block_env.number.saturating_to::<u64>();
        let mut cfg_env = segment.evm_env.cfg_env.clone();
        cfg_env.disable_base_fee = true;

        let inner = self.inner.as_mut().expect("inner executor must exist");
        let evm_ctx = inner.evm.ctx_mut();
        evm_ctx.block = block_env;
        evm_ctx.cfg = cfg_env;
        inner.ctx = segment.ctx.clone();

        self.reseed_block_hashes_for(block_number);

        Ok(())
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
        let hashes = self.plan.hashes_for_block(block_number);
        seeder(self.inner_mut().evm_mut().db_mut(), &hashes);
    }

    fn apply_segment_boundary(&mut self) -> Result<(), BlockExecutionError> {
        let plan = &mut self.plan;
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

        // Clone the next segment's data before we consume inner.
        let new_segment = &plan.segments[seg_idx];
        let new_block_env = new_segment.evm_env.block_env.clone();
        let mut new_cfg_env = new_segment.evm_env.cfg_env.clone();
        new_cfg_env.disable_base_fee = true;

        plan.next_segment += 1;
        plan.next_segment_start_tx =
            plan.segments.get(plan.next_segment).map_or(usize::MAX, |segment| segment.start_tx);

        // Finish the inner executor for the completed segment. This applies
        // post-execution system calls (EIP-7002/7251) and withdrawal balance
        // increments via EthBlockExecutor::finish() at the current bal_index
        // (= K, the boundary's "post-N slot").
        let mut inner = self.inner.take().expect("inner executor must exist");
        inner.ctx = prev_segment.ctx.clone();
        let spec = inner.spec.clone();
        let receipt_builder = inner.receipt_builder;

        if let Some(bumper) = self.bal_index_bumper {
            bumper(inner.evm_mut().db_mut());
        }

        let (mut evm, result) = inner.finish()?;

        // Renumbering: bump bal_index so the new segment's
        // `apply_pre_execution_changes` writes land at K+1 instead of colliding
        // with post-N at K. Without this, BAL workers querying at K can't see
        // either boundary write via `BalWrites::get`'s strict less-than.
        if let Some(bumper) = self.bal_index_bumper {
            bumper(evm.db_mut());
        }

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
        let mut new_inner =
            EthBlockExecutor::new(evm, new_segment.ctx.clone(), spec, receipt_builder);

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
        let new_block_number =
            self.plan.segments[seg_idx].evm_env.block_env.number.saturating_to::<u64>();
        self.reseed_block_hashes_for(new_block_number);

        // Apply pre-execution changes for the new segment (EIP-2935, EIP-4788)
        // at bal_index K+1.
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
        // Init swaps the EVM's block_env and executor ctx to the starting
        // segment's values so the EIP-2935/EIP-4788 system calls use the
        // correct block number and parent hash. Without this the outer big
        // block header's (synthetic) block_number would be used, writing to
        // wrong EIP-2935 slots and corrupting state.
        self.initialize()?;
        self.inner_mut().apply_pre_execution_changes()
    }

    fn execute_transaction_without_commit(
        &mut self,
        tx: impl ExecutableTx<Self>,
    ) -> Result<Self::Result, BlockExecutionError> {
        self.initialize()?;
        self.inner_mut().execute_transaction_without_commit(tx)
    }

    fn commit_transaction(&mut self, output: Self::Result) -> GasOutput {
        let offset = self.gas_used_offset;
        let gas_used = {
            let inner = self.inner_mut();
            let gas_used = inner.commit_transaction(output);

            // Fix up cumulative_gas_used on the just-committed receipt so that
            // the receipt root task (which reads receipts incrementally) sees
            // globally-correct values across all segments.
            if offset > 0 &&
                let Some(receipt) = inner.receipts.last_mut()
            {
                receipt.cumulative_gas_used += offset;
            }

            gas_used
        };

        self.plan.tx_counter += 1;

        while self.plan.tx_counter == self.plan.next_segment_start_tx {
            self.apply_segment_boundary().expect("must succeed");
        }

        gas_used
    }

    fn finish(
        mut self,
    ) -> Result<(Self::Evm, BlockExecutionResult<Self::Receipt>), BlockExecutionError> {
        // Swap the inner executor's ctx to the last segment's ctx so that
        // EthBlockExecutor::finish() applies the correct withdrawal balance
        // increments and post-execution system calls.
        let last_seg = self.plan.segments.last().unwrap();
        let last_ctx = EthBlockExecutionCtx {
            parent_hash: last_seg.ctx.parent_hash,
            parent_beacon_block_root: last_seg.ctx.parent_beacon_block_root,
            ommers: last_seg.ctx.ommers,
            withdrawals: last_seg.ctx.withdrawals.clone(),
            extra_data: last_seg.ctx.extra_data.clone(),
            tx_count_hint: last_seg.ctx.tx_count_hint,
            slot_number: last_seg.ctx.slot_number,
        };
        self.inner_mut().ctx = last_ctx;
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
        // Store the real hook in the shared slot and give the inner
        // executor a lightweight forwarder. This way the hook survives
        // inner executor finish/reconstruct cycles at segment boundaries.
        *self.shared_hook.lock().unwrap() = hook;
        let fwd = self.forwarding_hook();
        self.inner_mut().set_state_hook(fwd);
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
}

impl<Spec> BbBlockExecutorFactory<Spec> {
    pub const fn new(
        receipt_builder: RethReceiptBuilder,
        spec: Spec,
        evm_factory: EthEvmFactory,
    ) -> Self {
        Self { receipt_builder, spec, evm_factory }
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

    pub(crate) fn create_executor_with_seeder<'a, DB, I>(
        &'a self,
        evm: EthEvm<DB, I, PrecompilesMap>,
        plan: BbEvmPlan<'a>,
        block_hash_seeder: Option<BlockHashSeeder<DB>>,
        bal_index_reader: Option<BalIndexReader<DB>>,
        bal_index_bumper: Option<BalIndexBumper<DB>>,
        bal_index_setter: Option<BalIndexSetter<DB>>,
    ) -> BbBlockExecutor<'a, DB, I, PrecompilesMap, &'a Spec>
    where
        Spec: alloy_evm::eth::spec::EthExecutorSpec,
        DB: StateDB,
        I: Inspector<EthEvmContext<DB>>,
    {
        BbBlockExecutor::new(
            evm,
            plan,
            &self.spec,
            self.receipt_builder,
            block_hash_seeder,
            bal_index_reader,
            bal_index_bumper,
            bal_index_setter,
        )
    }
}

impl<Spec> BlockExecutorFactory for BbBlockExecutorFactory<Spec>
where
    Spec: alloy_evm::eth::spec::EthExecutorSpec + 'static,
    TxEnv: FromRecoveredTx<TransactionSigned> + FromTxWithEncoded<TransactionSigned>,
{
    type EvmFactory = EthEvmFactory;
    type ExecutionCtx<'a> = BbEvmPlan<'a>;
    type Transaction = TransactionSigned;
    type Receipt = Receipt;
    type TxExecutionResult = EthTxResult<
        <EthEvmFactory as EvmFactory>::HaltReason,
        <TransactionSigned as TransactionEnvelope>::TxType,
    >;
    type Executor<'a, DB: StateDB, I: Inspector<EthEvmContext<DB>>> =
        BbBlockExecutor<'a, DB, I, PrecompilesMap, &'a Spec>;

    fn evm_factory(&self) -> &Self::EvmFactory {
        &self.evm_factory
    }

    fn create_executor<'a, DB, I>(
        &'a self,
        evm: EthEvm<DB, I, PrecompilesMap>,
        ctx: BbEvmPlan<'a>,
    ) -> Self::Executor<'a, DB, I>
    where
        DB: StateDB,
        I: Inspector<EthEvmContext<DB>>,
    {
        BbBlockExecutor::new(evm, ctx, &self.spec, self.receipt_builder, None, None, None, None)
    }
}
