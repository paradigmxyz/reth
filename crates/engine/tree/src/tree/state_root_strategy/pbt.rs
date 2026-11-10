//! Experimental PBT strategy. Both commitments are maintained, using incremental durable
//! PBT checkpoints and Reth's normal trie updates for MPT. Execution state remains in memory.

use super::*;
use reth_evm::EvmEnv as _;
use reth_trie_pbt::{
    execution::apply_bundle, migration::mpt_root, persistence::SnapshotStore, Blake3Hasher,
    StateTree, TreeHasher,
};

/// Computes shadow PBT roots before activation and canonical PBT roots from activation on.
///
/// Install a verified anchor in the shared [`SnapshotStore`] before launching the node. The
/// anchor can be genesis for a PBT-from-genesis chain or a finalized MPT migration anchor.
/// Both trees are kept throughout operation, including activation-straddling reorgs. This
/// strategy does not configure staged/backfill sync, RPC proof formats, or snapshot discovery.
#[derive(Debug)]
pub struct PbtStateRootStrategy<H = Blake3Hasher> {
    store: Arc<SnapshotStore<H>>,
    activation_timestamp: u64,
    default: DefaultStateRootStrategy,
}

impl<H: TreeHasher> PbtStateRootStrategy<H> {
    /// Creates a strategy using a shared checkpoint store and an explicit fork timestamp.
    pub fn new(store: Arc<SnapshotStore<H>>, activation_timestamp: u64) -> Self {
        Self { store, activation_timestamp, default: DefaultStateRootStrategy::default() }
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
        if timestamp >= self.activation_timestamp {
            self.store.load(state_root).map_err(ProviderError::other)
        } else {
            let state = self.store.load_block(hash).map_err(ProviderError::other)?;
            if mpt_root(&state) != state_root {
                return Err(ProviderError::other(std::io::Error::other(
                    "PBT checkpoint does not match parent MPT commitment",
                )));
            }
            Ok(state)
        }
    }

    /// Returns the timestamp at which headers start committing to PBT roots.
    pub const fn activation_timestamp(&self) -> u64 {
        self.activation_timestamp
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
        let state = self.restore_parent(parent.hash(), parent.state_root(), parent.timestamp())?;
        let active = ctx.env().evm_env.block_env().timestamp.saturating_to::<u64>() >=
            self.activation_timestamp;
        Ok(PreparedStateRootJob::new(
            Box::new(PbtStateRootJob {
                mpt: SynchronousStateRootJob { state_provider_factory: ctx.state_provider_factory },
                parent: state,
                store: self.store.clone(),
                active,
            }),
            None,
        ))
    }

    fn prepare_payload_builder(
        &self,
        ctx: PayloadStateRootJobContext<'_, N, P>,
    ) -> ProviderResult<Option<PayloadStateRootHandle>> {
        if ctx.timestamp() < self.activation_timestamp {
            return <DefaultStateRootStrategy as StateRootStrategy<N, P, Evm>>::prepare_payload_builder(&self.default, ctx);
        }
        // Preparation errors are treated as an optional optimization failure by the engine.
        // Retain a failing output backend so a missing checkpoint cannot select an MPT root.
        let parent = self
            .restore_parent(
                ctx.parent_hash(),
                ctx.parent_state_root(),
                ctx.parent_header().timestamp(),
            )
            .map_err(|error| error.to_string());
        let store = self.store.clone();
        let factory = ctx.state_provider_factory;
        Ok(Some(PayloadStateRootHandle::from_output("pbt", move |bundle| {
            let parent = parent.as_ref().map_err(|error| {
                StateRootTaskError::Other(std::io::Error::other(error.clone()).into())
            })?;
            let state = apply_bundle(parent, bundle)
                .map_err(|err| StateRootTaskError::Other(err.into()))?;
            let provider = factory.database_provider_ro()?;
            let hashed_state = provider.hashed_post_state(bundle)?;
            let (_, trie_updates) = provider.state_root_with_updates(hashed_state.clone())?;
            store.save(&state).map_err(|err| StateRootTaskError::Other(err.into()))?;
            Ok(StateRootComputeOutcome {
                state_root: state.root(),
                trie_updates: Arc::new(trie_updates),
                hashed_state: Arc::new(hashed_state),
            })
        })))
    }
}

#[derive(Debug)]
struct PbtStateRootJob<N: NodePrimitives, P, H> {
    mpt: SynchronousStateRootJob<N, P>,
    parent: StateTree<H>,
    store: Arc<SnapshotStore<H>>,
    active: bool,
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
        let state = apply_bundle(&self.parent, &output.state).map_err(ProviderError::other)?;
        let mut outcome = self.mpt.finish(block, output, hashed_state)?;
        let mpt_root = outcome.state_root;
        if self.active {
            outcome.state_root = state.root();
        }
        if outcome.state_root == block.header().state_root() {
            self.store.save_block(block.hash(), &state).map_err(ProviderError::other)?;
        }
        debug!(target: "engine::pbt", block_hash = ?block.hash(), pbt_root = ?state.root(), ?mpt_root, active = self.active, "Computed state commitments");
        Ok(outcome)
    }
}
