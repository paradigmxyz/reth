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
//! - [`BbBlockExecutor`] wraps [`EthBlockExecutor`] and intercepts [`execute_transaction`] to apply
//!   segment-boundary changes.

use crate::evm_config::BigBlockSegment;
use alloy_consensus::Transaction;
use alloy_eips::Encodable2718;
use alloy_evm::{
    block::{
        state_changes::post_block_balance_increments, BlockExecutionError, BlockExecutionResult,
        BlockExecutor, BlockExecutorFactory, BlockExecutorFor, BlockValidationError, ExecutableTx,
        OnStateHook, StateDB,
    },
    eth::{
        receipt_builder::ReceiptBuilder, EthBlockExecutionCtx, EthBlockExecutor, EthEvmContext,
        EthTxResult,
    },
    precompiles::PrecompilesMap,
    Database, EthEvm, EthEvmFactory, Evm, EvmEnv, EvmFactory, FromRecoveredTx, FromTxWithEncoded,
};
use alloy_primitives::{Address, Bytes, Log, B256};
use revm::{
    context::{BlockEnv, TxEnv},
    context_interface::result::{EVMError, HaltReason, ResultAndState},
    database::DatabaseCommitExt,
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
    /// Whether EIP-4788 (Cancun beacon roots) is active.
    pub(crate) cancun_active: bool,
    /// Whether EIP-2935 (Prague block hashes) is active.
    pub(crate) prague_active: bool,
    /// Chain spec for computing withdrawal balance increments.
    pub(crate) chain_spec: Arc<dyn alloy_hardforks::EthereumHardforks + Send + Sync>,
    /// Block hashes to seed for inter-segment BLOCKHASH resolution.
    /// Includes both prior block hashes and inter-segment hashes.
    pub(crate) block_hashes_to_seed: Vec<(u64, B256)>,
}

impl BbEvmPlan {
    /// Creates a new `BbEvmPlan` from segments and hardfork flags.
    pub(crate) fn new(
        segments: Vec<BigBlockSegment>,
        cancun_active: bool,
        prague_active: bool,
        chain_spec: Arc<dyn alloy_hardforks::EthereumHardforks + Send + Sync>,
    ) -> Self {
        // Pre-compute all inter-segment block hashes.
        let mut block_hashes_to_seed = Vec::new();
        for seg_idx in 1..segments.len() {
            let seg = &segments[seg_idx];
            let finished_block_number = seg.evm_env.block_env.number.saturating_to::<u64>() - 1;
            let finished_block_hash = seg.ctx.parent_hash;
            block_hashes_to_seed.push((finished_block_number, finished_block_hash));
        }

        Self {
            segments,
            next_segment: 1,
            tx_counter: 0,
            cancun_active,
            prague_active,
            chain_spec,
            block_hashes_to_seed,
        }
    }
}

impl std::fmt::Debug for BbEvmPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BbEvmPlan")
            .field("segments", &self.segments)
            .field("next_segment", &self.next_segment)
            .field("tx_counter", &self.tx_counter)
            .field("cancun_active", &self.cancun_active)
            .field("prague_active", &self.prague_active)
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
    pub(crate) fn plan(&self) -> Option<&BbEvmPlan> {
        self.plan.as_ref()
    }

    /// Takes the plan out of this EVM, leaving `None` in its place.
    pub(crate) fn take_plan(&mut self) -> Option<BbEvmPlan> {
        self.plan.take()
    }

    /// Provides a mutable reference to the inner EVM context.
    pub(crate) fn ctx_mut(&mut self) -> &mut EthEvmContext<DB> {
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
    /// Staged plan to inject into the next created BbEvm.
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
pub(crate) struct BbBlockExecutor<'a, DB, I, P, Spec, R>
where
    DB: Database,
    R: ReceiptBuilder,
{
    inner: EthBlockExecutor<'a, BbEvm<DB, I, P>, Spec, R>,
    plan: Option<BbEvmPlan>,
}

impl<'a, DB, I, P, Spec, R> BbBlockExecutor<'a, DB, I, P, Spec, R>
where
    DB: StateDB,
    I: Inspector<EthEvmContext<DB>>,
    P: PrecompileProvider<EthEvmContext<DB>, Output = InterpreterResult>,
    Spec: alloy_evm::eth::spec::EthExecutorSpec + Clone,
    R: ReceiptBuilder,
{
    pub(crate) fn new(
        mut evm: BbEvm<DB, I, P>,
        ctx: EthBlockExecutionCtx<'a>,
        spec: Spec,
        receipt_builder: R,
    ) -> Self {
        let plan = evm.take_plan();
        let inner = EthBlockExecutor::new(evm, ctx, spec, receipt_builder);
        Self { inner, plan }
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
        let cancun_active = plan.cancun_active;
        let prague_active = plan.prague_active;

        debug!(
            target: "engine::bb::evm",
            seg_idx,
            tx_counter = plan.tx_counter,
            "Applying segment boundary"
        );

        // 1. Apply post-execution system calls (EIP-7002/7251) for the finished segment while its
        //    block env is still active.
        if prague_active {
            let _requests =
                self.inner.system_caller.apply_post_execution_changes(&mut self.inner.evm)?;
            trace!(target: "engine::bb::evm", "Applied post-execution system calls for finished segment");
        }

        // 2. Apply withdrawal balance increments for the finished segment.
        let prev_segment = &plan.segments[prev_seg_idx];
        let balance_increments = post_block_balance_increments(
            &*plan.chain_spec,
            &prev_segment.evm_env.block_env,
            prev_segment.ctx.ommers,
            prev_segment.ctx.withdrawals.as_deref(),
        );

        if !balance_increments.is_empty() {
            trace!(
                target: "engine::bb::evm",
                count = balance_increments.len(),
                "Applying withdrawal balance increments"
            );
            self.inner
                .evm
                .db_mut()
                .increment_balances(balance_increments)
                .map_err(|_| BlockValidationError::IncrementBalanceFailed)?;
        }

        // 3. Swap block and cfg environments.
        // Set gas_limit to u64::MAX to disable the per-segment gas check.
        // Re-execution in a big-block context changes state across segments,
        // which can cause transactions to consume different gas amounts than
        // in their original blocks. Using u64::MAX avoids spurious rejections
        // while keeping receipt cumulative_gas_used correct (it tracks actual
        // gas consumed, not the limit). The GASLIMIT opcode will return
        // u64::MAX, which is acceptable for a benchmarking tool.
        let new_segment = &plan.segments[seg_idx];
        let mut new_block_env = new_segment.evm_env.block_env.clone();
        new_block_env.gas_limit = u64::MAX;
        let mut new_cfg_env = new_segment.evm_env.cfg_env.clone();
        new_cfg_env.disable_base_fee = true;
        let parent_hash = new_segment.ctx.parent_hash;
        let parent_beacon_block_root = new_segment.ctx.parent_beacon_block_root;
        let block_number = new_segment.evm_env.block_env.number.saturating_to::<u64>();

        let ctx = self.inner.evm.ctx_mut();
        ctx.block = new_block_env;
        ctx.cfg = new_cfg_env;

        // 4. Apply EIP-2935 blockhashes contract call.
        if prague_active && block_number != 0 {
            self.inner
                .system_caller
                .apply_blockhashes_contract_call(parent_hash, &mut self.inner.evm)?;
            trace!(target: "engine::bb::evm", "Applied EIP-2935");
        }

        // 5. Apply EIP-4788 beacon root contract call.
        if cancun_active && block_number != 0 {
            self.inner
                .system_caller
                .apply_beacon_root_contract_call(parent_beacon_block_root, &mut self.inner.evm)?;
            trace!(target: "engine::bb::evm", "Applied EIP-4788");
        }

        plan.next_segment += 1;
        Ok(())
    }
}

impl<'a, DB, I, P, Spec, R> BlockExecutor for BbBlockExecutor<'a, DB, I, P, Spec, R>
where
    DB: StateDB,
    I: Inspector<EthEvmContext<DB>>,
    P: PrecompileProvider<EthEvmContext<DB>, Output = InterpreterResult>,
    Spec: alloy_evm::eth::spec::EthExecutorSpec + Clone,
    R: ReceiptBuilder<
        Transaction: Transaction + Encodable2718,
        Receipt: alloy_consensus::TxReceipt<Log = Log>,
    >,
    BbEvm<DB, I, P>: Evm<
        DB = DB,
        Tx = TxEnv,
        HaltReason = HaltReason,
        Error = EVMError<DB::Error>,
        Spec = SpecId,
        BlockEnv = BlockEnv,
    >,
    TxEnv:
        alloy_evm::FromRecoveredTx<R::Transaction> + alloy_evm::FromTxWithEncoded<R::Transaction>,
{
    type Transaction = R::Transaction;
    type Receipt = R::Receipt;
    type Evm = BbEvm<DB, I, P>;
    type Result =
        EthTxResult<HaltReason, <R::Transaction as alloy_consensus::TransactionEnvelope>::TxType>;

    fn apply_pre_execution_changes(&mut self) -> Result<(), BlockExecutionError> {
        // Swap the EVM's block_env and executor ctx to the first segment's
        // values so that the initial EIP-2935/EIP-4788 system calls use the
        // correct block number and parent hash. Without this, the outer big
        // block header's block_number (which is synthetic) would be used,
        // writing to wrong EIP-2935 slots and corrupting state.
        if let Some(plan) = &self.plan {
            let seg0 = &plan.segments[0];
            let mut block_env = seg0.evm_env.block_env.clone();
            block_env.gas_limit = u64::MAX;
            let mut cfg_env = seg0.evm_env.cfg_env.clone();
            cfg_env.disable_base_fee = true;

            let ctx = self.inner.evm.ctx_mut();
            ctx.block = block_env;
            ctx.cfg = cfg_env;

            self.inner.ctx = EthBlockExecutionCtx {
                parent_hash: seg0.ctx.parent_hash,
                parent_beacon_block_root: seg0.ctx.parent_beacon_block_root,
                ommers: seg0.ctx.ommers,
                withdrawals: seg0.ctx.withdrawals.clone(),
                extra_data: seg0.ctx.extra_data.clone(),
                tx_count_hint: seg0.ctx.tx_count_hint,
            };
        }

        self.inner.apply_pre_execution_changes()
    }

    fn execute_transaction_without_commit(
        &mut self,
        tx: impl ExecutableTx<Self>,
    ) -> Result<Self::Result, BlockExecutionError> {
        self.maybe_apply_boundary()?;
        self.inner.execute_transaction_without_commit(tx)
    }

    fn commit_transaction(&mut self, output: Self::Result) -> Result<u64, BlockExecutionError> {
        let gas_used = self.inner.commit_transaction(output)?;
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
        // increments. Without this, finish() would use the big block's outer
        // ctx which has different (wrong) withdrawals.
        if let Some(plan) = &self.plan {
            let last_seg = &plan.segments[plan.segments.len() - 1];
            self.inner.ctx = EthBlockExecutionCtx {
                parent_hash: last_seg.ctx.parent_hash,
                parent_beacon_block_root: last_seg.ctx.parent_beacon_block_root,
                ommers: last_seg.ctx.ommers,
                withdrawals: last_seg.ctx.withdrawals.clone(),
                extra_data: last_seg.ctx.extra_data.clone(),
                tx_count_hint: last_seg.ctx.tx_count_hint,
            };
        }
        self.inner.finish()
    }

    fn set_state_hook(&mut self, hook: Option<Box<dyn OnStateHook>>) {
        self.inner.set_state_hook(hook);
    }

    fn evm_mut(&mut self) -> &mut Self::Evm {
        self.inner.evm_mut()
    }

    fn evm(&self) -> &Self::Evm {
        self.inner.evm()
    }

    fn receipts(&self) -> &[Self::Receipt] {
        self.inner.receipts()
    }
}

// ---------------------------------------------------------------------------
// BbBlockExecutorFactory
// ---------------------------------------------------------------------------

/// Block executor factory that uses [`BbEvmFactory`] to produce [`BbEvm`]
/// instances and [`BbBlockExecutor`] for boundary-aware execution.
#[derive(Debug, Clone)]
pub struct BbBlockExecutorFactory<R, Spec> {
    receipt_builder: R,
    spec: Spec,
    evm_factory: BbEvmFactory,
}

impl<R, Spec> BbBlockExecutorFactory<R, Spec> {
    pub fn new(receipt_builder: R, spec: Spec, evm_factory: BbEvmFactory) -> Self {
        Self { receipt_builder, spec, evm_factory }
    }

    pub const fn evm_factory(&self) -> &BbEvmFactory {
        &self.evm_factory
    }

    pub const fn spec(&self) -> &Spec {
        &self.spec
    }

    pub const fn receipt_builder(&self) -> &R {
        &self.receipt_builder
    }
}

impl<R, Spec> BlockExecutorFactory for BbBlockExecutorFactory<R, Spec>
where
    R: ReceiptBuilder<
            Transaction: Transaction + Encodable2718,
            Receipt: alloy_consensus::TxReceipt<Log = Log>,
        > + 'static,
    Spec: alloy_evm::eth::spec::EthExecutorSpec + 'static,
    TxEnv: FromRecoveredTx<R::Transaction> + FromTxWithEncoded<R::Transaction>,
{
    type EvmFactory = BbEvmFactory;
    type ExecutionCtx<'a> = EthBlockExecutionCtx<'a>;
    type Transaction = R::Transaction;
    type Receipt = R::Receipt;

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
        BbBlockExecutor::new(evm, ctx, &self.spec, &self.receipt_builder)
    }
}
