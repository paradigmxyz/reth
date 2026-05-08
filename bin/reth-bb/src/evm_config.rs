//! Big-block EVM configuration.
//!
//! Wraps [`EthEvmConfig`] to create executors that handle multi-segment
//! big-block execution internally. At transaction boundaries defined by
//! [`BigBlockData`], the executor swaps the EVM environment (block env,
//! cfg env) and applies pre/post execution changes for each segment.

pub(crate) use reth_engine_primitives::BigBlockData;

use crate::{
    evm::{BalIndexReader, BbBlockExecutorFactory, BbEvmPlan},
    BigBlockMap,
};
use alloy_consensus::Header;
use alloy_evm::{
    eth::{spec::EthExecutorSpec, EthBlockExecutionCtx},
    EthEvmFactory,
};
use alloy_primitives::B256;
use alloy_rpc_types::engine::ExecutionData;
use core::convert::Infallible;
use reth_chainspec::{ChainSpec, EthChainSpec};
use reth_ethereum_forks::Hardforks;
use reth_ethereum_primitives::EthPrimitives;
use reth_evm::{
    ConfigureEngineEvm, ConfigureEvm, Database, EvmEnv, EvmEnvFor, ExecutableTxIterator,
    ExecutionCtxFor, NextBlockEnvAttributes,
};
use reth_evm_ethereum::{EthBlockAssembler, EthEvmConfig, RethReceiptBuilder};
use reth_primitives_traits::{SealedBlock, SealedHeader};
use revm::primitives::hardfork::SpecId;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Execution plan types
// ---------------------------------------------------------------------------

/// A single execution segment within a big block.
#[derive(Debug, Clone)]
pub(crate) struct BigBlockSegment {
    /// Transaction index at which this segment starts.
    pub start_tx: usize,
    /// The EVM environment for this segment.
    pub evm_env: EvmEnv,
    /// The execution context for this segment.
    pub ctx: EthBlockExecutionCtx<'static>,
}

// ---------------------------------------------------------------------------
// BbEvmConfig
// ---------------------------------------------------------------------------

/// EVM configuration for big-block execution.
///
/// Wraps [`EthEvmConfig`] and a shared [`BigBlockMap`]. When a big-block
/// payload is received, the plan is staged on the [`BbBlockExecutorFactory`]
/// and consumed when the executor is created. Block hashes for inter-segment
/// BLOCKHASH resolution are reseeded into `State::block_hashes` at each
/// segment boundary via a [`BlockHashSeeder`](crate::evm::BlockHashSeeder)
/// callback injected in [`ConfigureEvm::create_executor`].
#[derive(Debug, Clone)]
pub struct BbEvmConfig<C = ChainSpec> {
    /// The inner Ethereum EVM configuration (used for env computation).
    pub inner: EthEvmConfig<C>,
    /// Shared map of pending big-block metadata.
    pub pending: BigBlockMap,
    /// Block executor factory for big-block execution.
    executor_factory: BbBlockExecutorFactory<Arc<C>>,
    /// Block assembler.
    block_assembler: EthBlockAssembler<C>,
}

impl<C> BbEvmConfig<C> {
    /// Creates a new big-block EVM configuration.
    pub fn new(inner: EthEvmConfig<C>, pending: BigBlockMap) -> Self
    where
        C: Clone,
    {
        let chain_spec = inner.chain_spec().clone();
        let executor_factory = BbBlockExecutorFactory::new(
            RethReceiptBuilder::default(),
            chain_spec,
            EthEvmFactory::default(),
        );
        let block_assembler = inner.block_assembler.clone();

        Self { inner, pending, executor_factory, block_assembler }
    }
}

// ---------------------------------------------------------------------------
// Block hash seeder for State<DB>
// ---------------------------------------------------------------------------

/// Reseeds `State::block_hashes` with the given hashes.
///
/// This is used as a [`BlockHashSeeder`](crate::evm::BlockHashSeeder) callback,
/// injected into [`BbBlockExecutor`](crate::evm::BbBlockExecutor) from
/// `ConfigureEvm::create_executor` where the concrete `State<DB>` type is known.
/// At each segment boundary the executor calls this to populate the ring buffer
/// with the 256 block hashes relevant to the new segment's block number window.
fn seed_state_block_hashes<DB>(state: &mut &mut revm::database::State<DB>, hashes: &[(u64, B256)]) {
    for &(number, hash) in hashes {
        state.block_hashes.insert(number, hash);
    }
}

/// Reads the BAL index from a `&mut State<DB>`.
///
/// Used as a [`BalIndexReader`] callback so the
/// generic [`BbBlockExecutor`](crate::evm::BbBlockExecutor) can pick its
/// starting segment without a trait bound on `DB`.
fn read_bal_index<DB>(state: &&mut revm::database::State<DB>) -> u64 {
    state.bal_state.bal_index()
}

/// Bumps the BAL index on a `&mut State<DB>`.
///
/// Used as a [`BalIndexBumper`](crate::evm::BalIndexBumper) callback so the
/// generic [`BbBlockExecutor`](crate::evm::BbBlockExecutor) can advance
/// `bal_index` between sub-events of a segment boundary (post-N's `finish()`
/// and pre-N+1's `apply_pre_execution_changes()`) without a trait bound on
/// `DB`.
fn bump_bal_index<DB: revm::Database>(state: &mut &mut revm::database::State<DB>) {
    state.bump_bal_index();
}

/// Sets the BAL index on a `&mut State<DB>`.
///
/// Used as a [`BalIndexSetter`](crate::evm::BalIndexSetter) callback so
/// [`BbBlockExecutor::initialize`](crate::evm::BbBlockExecutor) can renumber
/// a worker's incoming `bal_index = i + 1` into the boundary-padded space
/// `i + 1 + 2*k` (where `k` is the worker's segment index).
fn set_bal_index<DB: revm::Database>(state: &mut &mut revm::database::State<DB>, index: u64) {
    state.set_bal_index(index);
}

// ---------------------------------------------------------------------------
// ConfigureEvm
// ---------------------------------------------------------------------------

impl<C> ConfigureEvm for BbEvmConfig<C>
where
    C: EthExecutorSpec + EthChainSpec<Header = Header> + Hardforks + 'static,
{
    type Primitives = EthPrimitives;
    type Error = Infallible;
    type NextBlockEnvCtx = NextBlockEnvAttributes;
    type BlockExecutorFactory = BbBlockExecutorFactory<Arc<C>>;
    type BlockAssembler = EthBlockAssembler<C>;

    fn block_executor_factory(&self) -> &Self::BlockExecutorFactory {
        &self.executor_factory
    }

    fn block_assembler(&self) -> &Self::BlockAssembler {
        &self.block_assembler
    }

    fn evm_env(&self, header: &Header) -> Result<EvmEnv<SpecId>, Self::Error> {
        self.inner.evm_env(header)
    }

    fn next_evm_env(
        &self,
        parent: &Header,
        attributes: &NextBlockEnvAttributes,
    ) -> Result<EvmEnv, Self::Error> {
        self.inner.next_evm_env(parent, attributes)
    }

    fn context_for_next_block(
        &self,
        parent: &SealedHeader,
        attributes: Self::NextBlockEnvCtx,
    ) -> Result<EthBlockExecutionCtx<'_>, Self::Error> {
        self.inner.context_for_next_block(parent, attributes)
    }

    fn create_executor<'a, DB, I>(
        &'a self,
        evm: reth_evm::EvmFor<Self, &'a mut revm::database::State<DB>, I>,
        ctx: EthBlockExecutionCtx<'a>,
    ) -> alloy_evm::block::BlockExecutorFor<
        'a,
        Self::BlockExecutorFactory,
        &'a mut revm::database::State<DB>,
        I,
    >
    where
        DB: Database,
        I: reth_evm::InspectorFor<Self, &'a mut revm::database::State<DB>> + 'a,
    {
        let bal_index_reader: Option<BalIndexReader<&'a mut revm::database::State<DB>>> =
            Some(read_bal_index::<DB>);

        // Inject concrete function pointers that know the `State<DB>` type so
        // the generic executor can manipulate `bal_index` and reseed block
        // hashes without a trait bound on `DB`.
        self.executor_factory.create_executor_with_seeder(
            evm,
            ctx,
            Some(seed_state_block_hashes::<DB>),
            bal_index_reader,
            Some(bump_bal_index::<DB>),
            Some(set_bal_index::<DB>),
        )
    }

    fn context_for_block<'a>(
        &self,
        block: &'a SealedBlock<reth_ethereum_primitives::Block>,
    ) -> Result<EthBlockExecutionCtx<'a>, Self::Error> {
        // Refresh the staged plan based on this block's hash so subsequent
        // `create_executor` calls - both canonical and parallel BAL workers -
        // see the right plan. Cleared explicitly when this block isn't a
        // big-block payload, so a stale plan from a prior big block doesn't
        // leak into a regular block.
        match self.plan_for_payload_hash(&block.hash()) {
            Some(plan) => self.executor_factory.stage_plan(plan),
            None => self.executor_factory.clear_staged_plan(),
        }
        self.inner.context_for_block(block)
    }
}

// ---------------------------------------------------------------------------
// ConfigureEngineEvm — intercepts payload methods for big blocks
// ---------------------------------------------------------------------------

impl<C> ConfigureEngineEvm<ExecutionData> for BbEvmConfig<C>
where
    C: EthExecutorSpec + EthChainSpec<Header = Header> + Hardforks + 'static,
{
    fn evm_env_for_payload(&self, payload: &ExecutionData) -> Result<EvmEnvFor<Self>, Self::Error> {
        let payload_hash = payload.block_hash();
        let bb_data = {
            let pending = self.pending.lock().unwrap();
            pending.get(&payload_hash).cloned()
        };

        if let Some(bb_data) = bb_data {
            match self.build_plan(&bb_data) {
                Some(plan) => self.executor_factory.stage_plan(plan),
                None => self.executor_factory.clear_staged_plan(),
            }

            let Some((_, first_exec_data)) = bb_data.env_switches.first() else {
                return self.inner.evm_env_for_payload(payload)
            };

            // Compute the env from the first segment before the executor is created.
            let mut env = self.inner.evm_env_for_payload(first_exec_data)?;

            // Disable basefee validation: transactions from different
            // original blocks may have gas prices below the big block's
            // effective basefee.
            env.cfg_env.disable_base_fee = true;

            Ok(env)
        } else {
            self.executor_factory.clear_staged_plan();
            self.inner.evm_env_for_payload(payload)
        }
    }

    fn context_for_payload<'a>(
        &self,
        payload: &'a ExecutionData,
    ) -> Result<ExecutionCtxFor<'a, Self>, Self::Error> {
        self.inner.context_for_payload(payload)
    }

    fn tx_iterator_for_payload(
        &self,
        payload: &ExecutionData,
    ) -> Result<impl ExecutableTxIterator<Self>, Self::Error> {
        self.inner.tx_iterator_for_payload(payload)
    }
}

// ---------------------------------------------------------------------------
// Plan construction
// ---------------------------------------------------------------------------

impl<C> BbEvmConfig<C>
where
    C: EthExecutorSpec + EthChainSpec<Header = Header> + Hardforks + 'static,
{
    fn plan_for_payload_hash(&self, payload_hash: &B256) -> Option<BbEvmPlan> {
        let pending = self.pending.lock().unwrap();
        self.build_plan(pending.get(payload_hash)?)
    }

    fn build_plan(&self, bb: &BigBlockData<ExecutionData>) -> Option<BbEvmPlan> {
        if bb.env_switches.is_empty() {
            return None;
        }

        let segments: Vec<_> = bb
            .env_switches
            .iter()
            .map(|(start_tx, exec_data)| {
                let mut evm_env = self.inner.evm_env_for_payload(exec_data).unwrap();
                evm_env.cfg_env.disable_base_fee = true;
                let ctx = self.inner.context_for_payload(exec_data).unwrap();
                BigBlockSegment { start_tx: *start_tx, evm_env, ctx: clone_ctx(&ctx) }
            })
            .collect();

        let mut plan = BbEvmPlan::new(segments);
        plan.block_hashes_to_seed.extend(bb.prior_block_hashes.iter().copied());
        plan.block_hashes_to_seed.sort_unstable_by_key(|(n, _)| *n);
        Some(plan)
    }
}

fn clone_ctx<'a>(ctx: &EthBlockExecutionCtx<'_>) -> EthBlockExecutionCtx<'a> {
    EthBlockExecutionCtx {
        tx_count_hint: ctx.tx_count_hint,
        parent_hash: ctx.parent_hash,
        parent_beacon_block_root: ctx.parent_beacon_block_root,
        ommers: &[],
        withdrawals: ctx.withdrawals.clone().map(|w| std::borrow::Cow::Owned(w.into_owned())),
        extra_data: ctx.extra_data.clone(),
        slot_number: ctx.slot_number,
    }
}
