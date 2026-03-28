//! Big-block EVM configuration.
//!
//! Wraps [`EthEvmConfig`] to create executors that handle multi-segment
//! big-block execution internally. At transaction boundaries defined by
//! [`BigBlockData`], the executor swaps the EVM environment (block env,
//! cfg env) and applies pre/post execution changes for each segment.

pub(crate) use reth_engine_primitives::BigBlockData;

use crate::{
    evm::{BbBlockExecutorFactory, BbEvmFactory, BbEvmPlan},
    BigBlockMap,
};
use alloy_consensus::Header;
use alloy_evm::eth::EthBlockExecutionCtx;
use alloy_primitives::B256;
use alloy_rpc_types::engine::ExecutionData;
use core::convert::Infallible;
use reth_chainspec::{ChainSpec, EthChainSpec};
use reth_ethereum_forks::Hardforks;
use reth_ethereum_primitives::EthPrimitives;
use reth_evm::{
    ConfigureEngineEvm, ConfigureEvm, Database, Evm as _, EvmEnv, ExecutableTxIterator,
    NextBlockEnvAttributes,
};
use reth_evm_ethereum::{EthBlockAssembler, EthEvmConfig, RethReceiptBuilder};
use reth_primitives_traits::{SealedBlock, SealedHeader};
use revm::primitives::hardfork::SpecId;
use std::sync::Arc;
use tracing::{debug, trace};

use alloy_evm::{block::BlockExecutorFactory as _, eth::spec::EthExecutorSpec, EthEvmFactory};
use reth_evm::{EvmEnvFor, ExecutionCtxFor};

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
/// payload is received, the plan is staged on the [`BbEvmFactory`] and
/// consumed during EVM creation. Block hashes for inter-segment BLOCKHASH
/// resolution are seeded into the `State` database via the overridden
/// [`ConfigureEvm::evm_with_env`].
#[derive(Debug, Clone)]
pub struct BbEvmConfig<C = ChainSpec> {
    /// The inner Ethereum EVM configuration (used for env computation).
    pub inner: EthEvmConfig<C>,
    /// Shared map of pending big-block metadata.
    pub pending: BigBlockMap,
    /// Block executor factory with the big-block EVM factory.
    executor_factory: BbBlockExecutorFactory<RethReceiptBuilder, Arc<C>>,
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
        let evm_factory = BbEvmFactory::new(EthEvmFactory::default());
        let executor_factory =
            BbBlockExecutorFactory::new(RethReceiptBuilder::default(), chain_spec, evm_factory);
        let block_assembler = inner.block_assembler.clone();

        Self { inner, pending, executor_factory, block_assembler }
    }
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
    type BlockExecutorFactory = BbBlockExecutorFactory<RethReceiptBuilder, Arc<C>>;
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

    fn context_for_block<'a>(
        &self,
        block: &'a SealedBlock<reth_ethereum_primitives::Block>,
    ) -> Result<EthBlockExecutionCtx<'a>, Self::Error> {
        self.inner.context_for_block(block)
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
        mut evm: reth_evm::EvmFor<Self, &'a mut revm::database::State<DB>, I>,
        ctx: EthBlockExecutionCtx<'a>,
    ) -> impl alloy_evm::block::BlockExecutorFor<
        'a,
        Self::BlockExecutorFactory,
        &'a mut revm::database::State<DB>,
        I,
    >
    where
        DB: Database,
        I: reth_evm::InspectorFor<Self, &'a mut revm::database::State<DB>> + 'a,
    {
        // Seed block hashes from the plan into State::block_hashes before
        // the executor is created. We have access to State<DB> here because
        // create_executor explicitly takes &mut State<DB> as the DB type.
        if let Some(plan) = evm.plan() {
            let hashes: Vec<_> = plan.block_hashes_to_seed.clone();
            for (number, hash) in hashes {
                evm.db_mut().block_hashes.insert(number, hash);
                trace!(
                    target: "engine::bb::evm",
                    number,
                    ?hash,
                    "Seeded block hash into State"
                );
            }
        }

        self.block_executor_factory().create_executor(evm, ctx)
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
        let has_plan = self.pending.lock().unwrap().contains_key(&payload_hash);

        if has_plan {
            // Compute the env from the first segment BEFORE removing the
            // entry (stage_plan_for_payload removes it).
            let first_exec_data = {
                let pending = self.pending.lock().unwrap();
                let bb_data = pending.get(&payload_hash).unwrap();
                bb_data.env_switches[0].1.clone()
            };
            let mut env = self.inner.evm_env_for_payload(&first_exec_data)?;

            // Disable basefee validation: transactions from different
            // original blocks may have gas prices below the big block's
            // effective basefee.
            env.cfg_env.disable_base_fee = true;

            // Disable the per-tx gas_limit check: set gas_limit high enough
            // that no transaction is rejected. Each segment has its own real
            // gas_limit, but the inflated values at boundaries handle that.
            // We just need the initial value to be large enough for the
            // first segment's transactions.
            env.block_env.gas_limit = u64::MAX;

            // Now stage the plan on the factory (removes the entry).
            self.stage_plan_for_payload(&payload_hash);

            Ok(env)
        } else {
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
// Plan construction and staging
// ---------------------------------------------------------------------------

impl<C> BbEvmConfig<C>
where
    C: EthExecutorSpec + EthChainSpec<Header = Header> + Hardforks + 'static,
{
    /// Takes the big-block plan for a payload hash, builds a [`BbEvmPlan`],
    /// and stages it on the factory.
    ///
    /// Must be called before `evm_with_env` is invoked for this payload.
    /// In practice, this is called from `evm_env_for_payload` in the
    /// engine pipeline.
    pub fn stage_plan_for_payload(&self, payload_hash: &B256) {
        let bb = match self.pending.lock().unwrap().remove(payload_hash) {
            Some(bb) => bb,
            None => return,
        };

        let segments: Vec<_> = bb
            .env_switches
            .into_iter()
            .map(|(start_tx, exec_data)| {
                let evm_env = self.inner.evm_env_for_payload(&exec_data).unwrap();
                let ctx = self.inner.context_for_payload(&exec_data).unwrap();
                let ctx = EthBlockExecutionCtx {
                    tx_count_hint: ctx.tx_count_hint,
                    parent_hash: ctx.parent_hash,
                    parent_beacon_block_root: ctx.parent_beacon_block_root,
                    ommers: &[],
                    withdrawals: ctx.withdrawals.map(|w| std::borrow::Cow::Owned(w.into_owned())),
                    extra_data: ctx.extra_data,
                };
                BigBlockSegment { start_tx, evm_env, ctx }
            })
            .collect();

        debug!(
            target: "engine::bb",
            ?payload_hash,
            segments = segments.len(),
            seed_hashes = bb.prior_block_hashes.len(),
            "Staging multi-segment plan"
        );

        // Determine hardfork activation.
        let chain_spec = self.inner.chain_spec().clone();
        let first_env = &segments[0].evm_env;
        let spec_id = *first_env.spec_id();
        let cancun_active = spec_id >= SpecId::CANCUN;
        let prague_active = spec_id >= SpecId::PRAGUE;

        let mut plan = BbEvmPlan::new(segments, cancun_active, prague_active, chain_spec);

        // Add prior block hashes to the seeding list.
        plan.block_hashes_to_seed.extend(bb.prior_block_hashes);

        // The BlockHashCache is a fixed-size 256-slot array indexed by
        // `block_number % 256`. When more than 256 hashes are seeded,
        // entries with colliding indices overwrite each other. Keep only
        // the 256 highest block numbers since BLOCKHASH only looks back
        // 256 blocks from the current block.
        plan.block_hashes_to_seed.sort_unstable_by_key(|(n, _)| *n);
        if plan.block_hashes_to_seed.len() > 256 {
            let excess = plan.block_hashes_to_seed.len() - 256;
            plan.block_hashes_to_seed.drain(..excess);
        }

        self.executor_factory.evm_factory().stage_plan(plan);
    }
}
