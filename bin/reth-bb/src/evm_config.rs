//! Big-block EVM configuration.
//!
//! Wraps [`EthEvmConfig`] to create executors that handle multi-segment
//! big-block execution internally. At transaction boundaries defined by
//! [`BigBlockData`], the executor swaps the EVM environment (block env,
//! cfg env) and applies pre/post execution changes for each segment.

use alloy_primitives::Bytes;
pub(crate) use reth_engine_primitives::BigBlockData;
use reth_engine_primitives::ExecutionPayload as _;
use reth_storage_errors::any::AnyError;

use crate::evm::{BalIndexReader, BbBlockExecutorFactory, BbEvmPlan};
use alloy_consensus::Header;
use alloy_eips::Decodable2718;
use alloy_evm::{
    eth::{spec::EthExecutorSpec, EthBlockExecutionCtx},
    EthEvmFactory,
};
use alloy_primitives::B256;
use alloy_rpc_types::engine::ExecutionData;
use core::convert::Infallible;
use reth_chainspec::{ChainSpec, EthChainSpec};
use reth_ethereum_forks::Hardforks;
use reth_ethereum_primitives::{Block, EthPrimitives};
use reth_evm::{
    execute::BlockAssembler, ConfigureEngineEvm, ConfigureEvm, Database, EvmEnv, EvmEnvFor,
    ExecutableTxIterator, ExecutionCtxFor, NextBlockEnvAttributes,
};
use reth_evm_ethereum::{EthEvmConfig, RethReceiptBuilder};
use reth_primitives_traits::{SealedBlock, SealedHeader, SignedTransaction, TxTy};
use revm::{primitives::hardfork::SpecId, state::bal::BlockAccessIndex};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Execution plan types
// ---------------------------------------------------------------------------

/// A single execution segment within a big block.
#[derive(Debug, Clone)]
pub(crate) struct BigBlockSegment<'a> {
    /// Transaction index at which this segment starts.
    pub start_tx: usize,
    /// The EVM environment for this segment.
    pub evm_env: EvmEnv,
    /// The execution context for this segment.
    pub ctx: EthBlockExecutionCtx<'a>,
}

// ---------------------------------------------------------------------------
// BbEvmConfig
// ---------------------------------------------------------------------------

/// EVM configuration for big-block execution.
///
/// Wraps [`EthEvmConfig`]. When a big-block payload is received, the plan is
/// staged on the [`BbBlockExecutorFactory`]
/// and consumed when the executor is created. Block hashes for inter-segment
/// BLOCKHASH resolution are reseeded into `State::block_hashes` at each
/// segment boundary via a [`BlockHashSeeder`](crate::evm::BlockHashSeeder)
/// callback injected in [`ConfigureEvm::create_executor`].
#[derive(Debug, Clone)]
pub struct BbEvmConfig<C = ChainSpec> {
    /// The inner Ethereum EVM configuration (used for env computation).
    pub inner: EthEvmConfig<C>,
    /// Block executor factory for big-block execution.
    executor_factory: BbBlockExecutorFactory<Arc<C>>,
    /// Block assembler.
    block_assembler: BbBlockAssembler,
}

impl<C> BbEvmConfig<C> {
    /// Creates a new big-block EVM configuration.
    pub fn new(inner: EthEvmConfig<C>) -> Self
    where
        C: Clone,
    {
        let chain_spec = inner.chain_spec().clone();
        let executor_factory = BbBlockExecutorFactory::new(
            RethReceiptBuilder::default(),
            chain_spec,
            EthEvmFactory::default(),
        );

        Self { inner, executor_factory, block_assembler: Default::default() }
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
const fn read_bal_index<DB>(state: &&mut revm::database::State<DB>) -> u64 {
    state.bal_state.bal_index().get()
}

/// Bumps the BAL index on a `&mut State<DB>`.
///
/// Used as a [`BalIndexBumper`](crate::evm::BalIndexBumper) callback so the
/// generic [`BbBlockExecutor`](crate::evm::BbBlockExecutor) can advance
/// `bal_index` between sub-events of a segment boundary (post-N's `finish()`
/// and pre-N+1's `apply_pre_execution_changes()`) without a trait bound on
/// `DB`.
const fn bump_bal_index<DB: revm::Database>(state: &mut &mut revm::database::State<DB>) {
    state.bump_bal_index();
}

/// Sets the BAL index on a `&mut State<DB>`.
///
/// Used as a [`BalIndexSetter`](crate::evm::BalIndexSetter) callback so
/// [`BbBlockExecutor::initialize`](crate::evm::BbBlockExecutor) can renumber
/// a worker's incoming `bal_index = i + 1` into the boundary-padded space
/// `i + 1 + 2*k` (where `k` is the worker's segment index).
const fn set_bal_index<DB: revm::Database>(state: &mut &mut revm::database::State<DB>, index: u64) {
    state.set_bal_index(BlockAccessIndex::new(index));
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
    type BlockAssembler = BbBlockAssembler;

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
        _block: &'a SealedBlock<reth_ethereum_primitives::Block>,
    ) -> Result<BbEvmPlan<'a>, Self::Error> {
        unreachable!("big blocks EVM should only be used from within the engine pipeline")
    }

    fn context_for_next_block(
        &self,
        _parent: &SealedHeader,
        _attributes: Self::NextBlockEnvCtx,
    ) -> Result<BbEvmPlan<'_>, Self::Error> {
        unreachable!("big blocks EVM should only be used from within the engine pipeline")
    }

    fn create_executor<'a, DB, I>(
        &'a self,
        evm: reth_evm::EvmFor<Self, &'a mut revm::database::State<DB>, I>,
        ctx: BbEvmPlan<'a>,
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

    fn create_executor_with_state<'ctx, 'db, DB, I>(
        &'ctx self,
        evm: reth_evm::EvmFor<Self, &'db mut revm::database::State<DB>, I>,
        ctx: BbEvmPlan<'ctx>,
    ) -> alloy_evm::block::BlockExecutorFor<
        'ctx,
        Self::BlockExecutorFactory,
        &'db mut revm::database::State<DB>,
        I,
    >
    where
        DB: Database,
        I: reth_evm::InspectorFor<Self, &'db mut revm::database::State<DB>>,
    {
        let bal_index_reader: Option<BalIndexReader<&'db mut revm::database::State<DB>>> =
            Some(read_bal_index::<DB>);

        self.executor_factory.create_executor_with_seeder(
            evm,
            ctx,
            Some(seed_state_block_hashes::<DB>),
            bal_index_reader,
            Some(bump_bal_index::<DB>),
            Some(set_bal_index::<DB>),
        )
    }
}

// ---------------------------------------------------------------------------
// ConfigureEngineEvm — intercepts payload methods for big blocks
// ---------------------------------------------------------------------------

impl<C> ConfigureEngineEvm<BigBlockData<ExecutionData>> for BbEvmConfig<C>
where
    C: EthExecutorSpec + EthChainSpec<Header = Header> + Hardforks + 'static,
{
    fn evm_env_for_payload(
        &self,
        payload: &BigBlockData<ExecutionData>,
    ) -> Result<EvmEnvFor<Self>, Self::Error> {
        // Compute the env from the first segment BEFORE removing the
        // entry (stage_plan_for_payload removes it).
        let first_exec_data = &payload.env_switches[0];
        let mut env = self.inner.evm_env_for_payload(first_exec_data)?;

        // Disable basefee validation: transactions from different
        // original blocks may have gas prices below the big block's
        // effective basefee.
        env.cfg_env.disable_base_fee = true;
        env.block_env.gas_limit = payload.gas_limit();

        Ok(env)
    }

    fn context_for_payload<'a>(
        &self,
        payload: &'a BigBlockData<ExecutionData>,
    ) -> Result<ExecutionCtxFor<'a, Self>, Self::Error> {
        let mut current_tx = 0;
        let segments: Vec<_> = payload
            .env_switches
            .iter()
            .map(|exec_data| {
                let start_tx = current_tx;
                let mut evm_env = self.inner.evm_env_for_payload(exec_data)?;
                evm_env.cfg_env.disable_base_fee = true;
                let ctx = self.inner.context_for_payload(exec_data)?;
                current_tx += exec_data.payload.transactions().len();
                Ok(BigBlockSegment { start_tx, evm_env, ctx })
            })
            .collect::<Result<_, Self::Error>>()?;

        let mut plan = BbEvmPlan::new(segments);

        // Add prior block hashes to the seeding list.
        plan.block_hashes_to_seed.extend(payload.prior_block_hashes.clone());
        plan.block_hashes_to_seed.sort_unstable_by_key(|(n, _)| *n);

        Ok(plan)
    }

    fn tx_iterator_for_payload(
        &self,
        payload: &BigBlockData<ExecutionData>,
    ) -> Result<impl ExecutableTxIterator<Self>, Self::Error> {
        let transactions = payload
            .env_switches
            .iter()
            .flat_map(|exec_data| exec_data.payload.transactions().clone())
            .collect::<Vec<_>>();

        let convert = |tx: Bytes| {
            let tx =
                TxTy::<Self::Primitives>::decode_2718_exact(tx.as_ref()).map_err(AnyError::new)?;
            let signer = tx.try_recover().map_err(AnyError::new)?;
            Ok::<_, AnyError>(tx.with_signer(signer))
        };

        Ok((transactions, convert))
    }
}

#[derive(Debug, Default, Clone)]
pub struct BbBlockAssembler;

impl<Spec: EthExecutorSpec + 'static> BlockAssembler<BbBlockExecutorFactory<Spec>>
    for BbBlockAssembler
{
    type Block = Block;

    fn assemble_block(
        &self,
        _input: reth_evm::execute::BlockAssemblerInput<
            '_,
            '_,
            BbBlockExecutorFactory<Spec>,
            <Self::Block as reth_node_api::Block>::Header,
        >,
    ) -> Result<Self::Block, reth_errors::BlockExecutionError> {
        unreachable!("block building is not supported for big blocks EVM")
    }
}
