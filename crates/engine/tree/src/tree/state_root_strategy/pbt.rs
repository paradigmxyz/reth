//! PBT commitments with MPT maintenance until finalized activation retires its node storage.

use super::*;
use reth_evm::EvmEnv as _;
use reth_trie_pbt::{
    execution::{apply_bundle, apply_bundle_parallel},
    migration::mpt_root,
    persistence::SnapshotStore,
    replay::{replay, replay_parallel},
    Blake3Hasher, StateTree, TreeHasher,
};
use tracing::info;

pub use alloy_eip7928::AccountChanges;
pub use reth_trie_pbt::replay::ReplayError;

/// Computes shadow PBT roots before activation and canonical PBT roots from activation on.
///
/// Install a verified anchor in the shared [`SnapshotStore`] before launching the node. The
/// anchor can be genesis for a PBT-from-genesis chain or a finalized MPT migration anchor.
/// Both trees are kept through the transition window, including activation-straddling reorgs.
/// After the node retires MPT storage, validation and payload building compute only PBT roots.
/// Install the node's matching staged/backfill and retirement integration as well.
#[derive(Debug)]
pub struct PbtStateRootStrategy<H = Blake3Hasher> {
    store: Arc<SnapshotStore<H>>,
    activation_timestamp: u64,
    default: DefaultStateRootStrategy,
    parallel: bool,
}

impl<H: TreeHasher> PbtStateRootStrategy<H> {
    /// Creates a strategy using a shared checkpoint store and an explicit fork timestamp.
    pub fn new(store: Arc<SnapshotStore<H>>, activation_timestamp: u64) -> Self {
        Self {
            store,
            activation_timestamp,
            default: DefaultStateRootStrategy::default(),
            parallel: false,
        }
    }

    /// Returns the shared snapshot store for shadow-root inspection or replay onboarding.
    pub const fn store(&self) -> &Arc<SnapshotStore<H>> {
        &self.store
    }

    /// Restores a trusted header's state, authenticating the pre-activation MPT commitment
    /// or the post-activation PBT commitment. Call this before resuming a persisted chain;
    /// a missing or mismatched checkpoint is an error, never an MPT fallback.
    pub fn restore_parent(
        &self,
        hash: B256,
        state_root: B256,
        timestamp: u64,
    ) -> ProviderResult<StateTree<H>> {
        let started = Instant::now();
        let state = if timestamp >= self.activation_timestamp {
            self.store.load(state_root).map_err(ProviderError::other)?
        } else {
            let state = self.store.load_block(hash).map_err(ProviderError::other)?;
            if mpt_root(&state) != state_root {
                return Err(ProviderError::other(std::io::Error::other(
                    "PBT checkpoint does not match parent MPT commitment",
                )));
            }
            state
        };
        info!(
            target: "engine::pbt",
            phase = "restore",
            block_hash = ?hash,
            parallel = self.parallel,
            leaves = state.tree().len(),
            elapsed_ms = started.elapsed().as_secs_f64() * 1000.0,
            "PBT parent restore timings"
        );
        Ok(state)
    }

    /// Returns the timestamp at which headers start committing to PBT roots.
    pub const fn activation_timestamp(&self) -> u64 {
        self.activation_timestamp
    }

    /// Rejects a parent below the permanent retirement boundary or before PBT activation.
    ///
    /// Pass the marker from the same database view used for state-root work. `None` leaves
    /// activation-straddling reorgs available throughout the transition window.
    pub fn validate_retired_parent(
        &self,
        parent: &impl AlloyBlockHeader,
        retired_at: Option<u64>,
    ) -> ProviderResult<()> {
        if retired_at.is_some_and(|number| {
            parent.number() < number || parent.timestamp() < self.activation_timestamp
        }) {
            return Err(ProviderError::other(std::io::Error::other(
                "cannot unwind past MPT retirement; re-anchor or resync",
            )))
        }
        Ok(())
    }

    /// Enables parallel hashing for dense block and BAL state-update batches.
    /// Small or sparse changes retain incremental updates; commitment rules stay identical.
    pub const fn with_parallel(mut self, parallel: bool) -> Self {
        self.parallel = parallel;
        self
    }

    /// Replays a BAL authenticated against the caller's validated header commitment.
    /// Uses this strategy's hashing mode and preserves the parent on every error.
    pub fn replay(
        &self,
        parent: &StateTree<H>,
        bal: &[AccountChanges],
        expected_bal_hash: B256,
    ) -> Result<StateTree<H>, ReplayError> {
        let started = Instant::now();
        let state = if self.parallel {
            replay_parallel(parent, bal, expected_bal_hash)?
        } else {
            replay(parent, bal, expected_bal_hash)?
        };
        info!(
            target: "engine::pbt",
            phase = "replay",
            parallel = self.parallel,
            accounts = bal.len(),
            leaves = state.tree().len(),
            pbt_root = ?state.root(),
            pbt_update_ms = started.elapsed().as_secs_f64() * 1000.0,
            "PBT BAL replay timings"
        );
        Ok(state)
    }
}

impl<N, P, Evm, H> StateRootStrategy<N, P, Evm> for PbtStateRootStrategy<H>
where
    N: NodePrimitives,
    P: DatabaseProviderFactory + Clone + 'static,
    P::Provider: BlockNumReader
        + PruneCheckpointReader
        + StageCheckpointReader
        + ChangeSetReader
        + StorageChangeSetReader
        + StorageSettingsCache
        + 'static,
    Evm: ConfigureEvm<Primitives = N> + 'static,
    H: TreeHasher,
{
    fn prepare(
        &self,
        ctx: StateRootJobContext<'_, N, P, Evm>,
    ) -> ProviderResult<PreparedStateRootJob<N>> {
        let parent = ctx.parent_header();
        let retired = ctx.state_provider_factory.database_provider_ro()?.mpt_retired_at()?;
        self.validate_retired_parent(parent.header(), retired)?;
        let active = ctx.env().evm_env.block_env().timestamp.saturating_to::<u64>() >=
            self.activation_timestamp;
        let state = if !active && parent.timestamp() < self.activation_timestamp {
            // These checkpoints derive from an authenticated anchor and locally accepted
            // execution. Re-authenticate at startup and activation, not every warm read.
            self.store.load_block(parent.hash()).map_err(ProviderError::other)?
        } else {
            self.restore_parent(parent.hash(), parent.state_root(), parent.timestamp())?
        };
        let mpt_parent = if retired.is_none() {
            Some(if parent.timestamp() < self.activation_timestamp {
                parent.state_root()
            } else {
                self.store.shadow_mpt_root(&state).map_err(ProviderError::other)?
            })
        } else {
            None
        };
        let job = PbtStateRootJob {
            state_provider_factory: ctx.state_provider_factory.clone(),
            mpt: None,
            parent: state,
            store: self.store.clone(),
            active,
            parallel: self.parallel,
        };
        if let Some(root) = mpt_parent {
            let prepared = <DefaultStateRootStrategy as StateRootStrategy<N, P, Evm>>::prepare(
                &self.default,
                if active { ctx.with_parent_state_root(root) } else { ctx },
            )?;
            Ok(PreparedStateRootJob {
                job: Box::new(PbtStateRootJob { mpt: Some(prepared.job), ..job }),
                ..prepared
            })
        } else {
            Ok(PreparedStateRootJob::new(Box::new(job), None))
        }
    }

    fn prepare_payload_builder(
        &self,
        ctx: PayloadStateRootJobContext<'_, N, P>,
    ) -> ProviderResult<Option<PayloadStateRootHandle>> {
        let retired = ctx
            .state_provider_factory
            .database_provider_ro()
            .and_then(|provider| provider.mpt_retired_at());
        if ctx.timestamp() < self.activation_timestamp && matches!(&retired, Ok(None)) {
            return <DefaultStateRootStrategy as StateRootStrategy<N, P, Evm>>::prepare_payload_builder(&self.default, ctx);
        }
        // Preparation errors are treated as an optional optimization failure by the engine.
        // Retain a failing output backend so a missing checkpoint cannot select an MPT root.
        let parent_number = ctx.parent_header().number();
        let timestamp = ctx.timestamp();
        let parallel = self.parallel;
        let parent = retired
            .and_then(|retired| {
                self.validate_retired_parent(ctx.parent_header(), retired)?;
                self.restore_parent(
                    ctx.parent_hash(),
                    ctx.parent_state_root(),
                    ctx.parent_header().timestamp(),
                )
            })
            .map_err(|error| error.to_string());
        let store = self.store.clone();
        let factory = ctx.state_provider_factory;
        Ok(Some(PayloadStateRootHandle::from_output("pbt", move |bundle| {
            let started = Instant::now();
            let parent = parent.as_ref().map_err(|error| {
                StateRootTaskError::Other(std::io::Error::other(error.clone()).into())
            })?;
            let update_started = Instant::now();
            let state = if parallel {
                apply_bundle_parallel(parent, bundle)
            } else {
                apply_bundle(parent, bundle)
            }
            .map_err(|err| StateRootTaskError::Other(err.into()))?;
            let update_elapsed = update_started.elapsed();
            let provider = factory.database_provider_ro()?;
            let retired = provider.mpt_retired_at()?;
            if retired.is_some_and(|number| parent_number < number) {
                return Err(StateRootTaskError::Other(
                    std::io::Error::other("cannot unwind past MPT retirement; re-anchor or resync")
                        .into(),
                ))
            }
            let hashed_state = provider.hashed_post_state(bundle)?;
            let mpt_started = Instant::now();
            let trie_updates = if retired.is_none() {
                provider.state_root_with_updates(hashed_state.clone())?.1
            } else {
                TrieUpdates::default()
            };
            let mpt_ms =
                if retired.is_none() { mpt_started.elapsed().as_secs_f64() * 1000.0 } else { 0.0 };
            let checkpoint_started = Instant::now();
            store.cache_candidate(&state).map_err(|err| StateRootTaskError::Other(err.into()))?;
            let checkpoint_elapsed = checkpoint_started.elapsed();
            info!(
                target: "engine::pbt",
                phase = "payload",
                parent_number,
                timestamp,
                parallel,
                maintain_mpt = retired.is_none(),
                accounts = bundle.state().len(),
                leaves = state.tree().len(),
                pbt_root = ?state.root(),
                pbt_update_ms = update_elapsed.as_secs_f64() * 1000.0,
                mpt_ms,
                checkpoint_ms = 0.0,
                candidate_cache_ms = checkpoint_elapsed.as_secs_f64() * 1000.0,
                total_ms = started.elapsed().as_secs_f64() * 1000.0,
                "PBT state root timings"
            );
            Ok(StateRootComputeOutcome {
                state_root: state.root(),
                trie_updates: Arc::new(trie_updates),
                hashed_state: Arc::new(hashed_state),
            })
        })))
    }

    fn requires_payload_output(&self, timestamp: u64) -> bool {
        timestamp >= self.activation_timestamp
    }
}

struct PbtStateRootJob<N: NodePrimitives, P, H> {
    state_provider_factory: OverlayStateProviderFactory<P, N>,
    mpt: Option<Box<dyn StateRootJob<N>>>,
    parent: StateTree<H>,
    store: Arc<SnapshotStore<H>>,
    active: bool,
    parallel: bool,
}

impl<N, P, H> StateRootJob<N> for PbtStateRootJob<N, P, H>
where
    N: NodePrimitives,
    P: DatabaseProviderFactory + Clone + 'static,
    P::Provider: BlockNumReader
        + PruneCheckpointReader
        + StageCheckpointReader
        + ChangeSetReader
        + StorageChangeSetReader
        + StorageSettingsCache
        + 'static,
    H: TreeHasher,
{
    fn name(&self) -> &'static str {
        "pbt"
    }

    fn finish(
        &mut self,
        block: &RecoveredBlock<N::Block>,
        output: Arc<BlockExecutionOutput<N::Receipt>>,
        hashed_state: &LazyHashedPostState,
    ) -> ProviderResult<StateRootJobOutcome> {
        let accounts = output.state.state().len();
        let started = Instant::now();
        let update_started = Instant::now();
        let state = if self.parallel {
            apply_bundle_parallel(&self.parent, &output.state)
        } else {
            apply_bundle(&self.parent, &output.state)
        }
        .map_err(ProviderError::other)?;
        let update_elapsed = update_started.elapsed();
        let retired = self.state_provider_factory.database_provider_ro()?.mpt_retired_at()?;
        if retired.is_some_and(|number| block.number() <= number || !self.active) {
            return Err(ProviderError::other(std::io::Error::other(
                "cannot unwind past MPT retirement; re-anchor or resync",
            )))
        }
        let mpt_started = Instant::now();
        let maintain_mpt = retired.is_none() && self.mpt.is_some();
        let mut outcome = if maintain_mpt && let Some(mpt) = &mut self.mpt {
            let outcome = mpt.finish(block, output, hashed_state)?;
            self.store
                .cache_shadow_mpt_root(state.root(), outcome.state_root)
                .map_err(ProviderError::other)?;
            outcome
        } else {
            StateRootJobOutcome::new(state.root(), Arc::new(TrieUpdates::default()))
        };
        let mpt_ms = if maintain_mpt { mpt_started.elapsed().as_secs_f64() * 1000.0 } else { 0.0 };
        let auxiliary_root = outcome.state_root;
        if self.active {
            outcome.state_root = state.root();
        }
        let checkpoint_started = Instant::now();
        let checkpointed = outcome.state_root == block.header().state_root();
        if checkpointed {
            if maintain_mpt && let Some(mpt) = &mut self.mpt {
                mpt.on_validated(block.hash(), auxiliary_root);
            }
            self.store
                .save_block_with_number(block.hash(), block.number(), &state)
                .map_err(ProviderError::other)?;
        }
        let checkpoint_ms =
            if checkpointed { checkpoint_started.elapsed().as_secs_f64() * 1000.0 } else { 0.0 };
        info!(
            target: "engine::pbt",
            phase = "validation",
            block_number = block.number(),
            block_hash = ?block.hash(),
            pbt_root = ?state.root(),
            parallel = self.parallel,
            maintain_mpt,
            active = self.active,
            accounts,
            leaves = state.tree().len(),
            pbt_update_ms = update_elapsed.as_secs_f64() * 1000.0,
            mpt_ms,
            checkpoint_ms,
            checkpointed,
            total_ms = started.elapsed().as_secs_f64() * 1000.0,
            "PBT state root timings"
        );
        Ok(outcome)
    }
}
