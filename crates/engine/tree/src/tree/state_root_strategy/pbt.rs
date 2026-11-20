//! PBT commitments with MPT maintenance until finalized activation retires its node storage.

use super::*;
use alloy_eip7928::bal::DecodedBal;
use futures::FutureExt as _;
use reth_evm::EvmEnv as _;
use reth_provider::{BalProvider, HeaderProvider, RawBal};
use reth_trie_pbt::{
    execution::{ExecutionParent, ExecutionState},
    migration::mpt_root,
    persistence::SnapshotStore,
    replay::{replay, replay_parallel},
    Blake3Hasher, StateTree, TreeHasher,
};
use std::{num::NonZeroUsize, time::Duration};
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
    default: Arc<DefaultStateRootStrategy>,
    parallel: bool,
    shadow_replay: Option<(NonZeroUsize, NonZeroUsize)>,
}

impl<H: TreeHasher> PbtStateRootStrategy<H> {
    /// Creates a strategy using a shared checkpoint store and an explicit fork timestamp.
    pub fn new(store: Arc<SnapshotStore<H>>, activation_timestamp: u64) -> Self {
        Self {
            store,
            activation_timestamp,
            default: Arc::default(),
            parallel: false,
            shadow_replay: None,
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

    /// Authenticates a pre-activation checkpoint against its accepted MPT commitment.
    /// The shadow worker warms the auxiliary-root cache before activation. A cache hit
    /// can then use a disk parent without restoring and hashing the complete state again.
    /// Cold or evicted entries still receive full authentication before being cached.
    pub fn authenticated_mpt_parent(
        &self,
        hash: B256,
        expected_mpt_root: B256,
    ) -> ProviderResult<ExecutionParent<H>> {
        let root = self.store.checkpoint_root(hash).map_err(ProviderError::other)?;
        let (parent, mpt_root) =
            self.store.execution_parent_with_shadow(hash, root).map_err(ProviderError::other)?;
        if mpt_root != expected_mpt_root {
            return Err(ProviderError::other(std::io::Error::other(
                "PBT checkpoint does not match parent MPT commitment",
            )))
        }
        Ok(parent)
    }

    /// Enables background shadow maintenance when the node runs the canonical checkpoint worker.
    /// These limits bound recovery if the worker is behind the activation parent.
    pub const fn with_background_shadow(
        mut self,
        max_blocks: NonZeroUsize,
        max_bal_bytes: NonZeroUsize,
    ) -> Self {
        self.shadow_replay = Some((max_blocks, max_bal_bytes));
        self
    }

    /// Materializes any missing pre-activation checkpoints on the branch ending at this header.
    /// In-memory branches use validated execution output. Persisted blocks use authenticated BALs.
    /// Headers and execution outputs must come from the node's validated chain.
    /// Both budgets apply to the complete recovery call; completed checkpoints remain
    /// available if the byte budget is exhausted before reaching the target.
    pub fn ensure_shadow_checkpoint<N, P>(
        &self,
        hash: B256,
        header: &N::BlockHeader,
        provider: &P,
        overlay: &OverlayManager<N>,
    ) -> ProviderResult<()>
    where
        N: NodePrimitives,
        P: BalProvider + HeaderProvider<Header = N::BlockHeader>,
    {
        let Some((max_blocks, max_bal_bytes)) = self.shadow_replay else { return Ok(()) };
        if self.store.contains_block(hash).map_err(ProviderError::other)? {
            return Ok(())
        }

        let mut hash = hash;
        let mut header = header.clone();
        let mut executed = overlay.executed_block_by_hash(&hash);
        let mut missing = Vec::new();
        loop {
            if missing.len() == max_blocks.get() ||
                header.number() == 0 ||
                header.timestamp() >= self.activation_timestamp
            {
                return Err(ProviderError::other(std::io::Error::other(
                    "missing PBT shadow checkpoint within the replay block budget",
                )))
            }
            let parent_hash = header.parent_hash();
            missing.push((hash, header, executed));
            if self.store.contains_block(parent_hash).map_err(ProviderError::other)? {
                break
            }
            hash = parent_hash;
            executed = overlay.executed_block_by_hash(&hash);
            header = if let Some(block) = &executed {
                block.recovered_block().header().clone()
            } else {
                provider
                    .sealed_header_by_hash(hash)?
                    .ok_or_else(|| ProviderError::HeaderNotFound(hash.into()))?
                    .into_header()
            };
        }

        let mut remaining_bal_bytes = max_bal_bytes.get();
        for (hash, header, executed) in missing.into_iter().rev() {
            if self.store.contains_block(hash).map_err(ProviderError::other)? {
                continue
            }
            let parent_hash = header.parent_hash();
            if let Some(block) = executed {
                let parent =
                    self.store.execution_parent(parent_hash, None).map_err(ProviderError::other)?;
                let state = parent
                    .apply_bundle(&block.execution_outcome().state, self.parallel)
                    .map_err(ProviderError::other)?;
                self.store
                    .save_execution_block(hash, header.number(), &state)
                    .map_err(ProviderError::other)?;
            } else {
                let commitment = header.block_access_list_hash().ok_or_else(|| {
                    ProviderError::other(std::io::Error::other("missing PBT shadow BAL commitment"))
                })?;
                let bytes = provider.bal_store().get_by_hash(hash)?.ok_or_else(|| {
                    ProviderError::other(std::io::Error::other("missing PBT shadow BAL"))
                })?;
                remaining_bal_bytes =
                    remaining_bal_bytes.checked_sub(bytes.len()).ok_or_else(|| {
                        ProviderError::other(std::io::Error::other(
                            "PBT shadow BAL byte budget exceeded",
                        ))
                    })?;
                let raw = RawBal::new(bytes);
                raw.ensure_hash(commitment).map_err(ProviderError::other)?;
                let bal = DecodedBal::from_raw_bal(raw).map_err(ProviderError::other)?;
                let parent = self.store.load_block(parent_hash).map_err(ProviderError::other)?;
                let state = self
                    .replay(&parent, bal.as_bal().as_slice(), commitment)
                    .map_err(ProviderError::other)?;
                self.store
                    .save_block_with_number(hash, header.number(), &state)
                    .map_err(ProviderError::other)?;
            }
        }
        Ok(())
    }

    fn execution_parent(
        &self,
        hash: B256,
        root: B256,
        parent_timestamp: u64,
        active: bool,
    ) -> ProviderResult<ExecutionParent<H>> {
        if parent_timestamp >= self.activation_timestamp {
            self.store.execution_parent(hash, Some(root)).map_err(ProviderError::other)
        } else if active {
            // The activation parent still commits to the MPT and needs dual-state authentication.
            self.authenticated_mpt_parent(hash, root)
        } else {
            self.store.execution_parent(hash, None).map_err(ProviderError::other)
        }
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

    /// Enables parallel hashing for dense eager state updates and BAL replay batches.
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
    P: BalProvider
        + DatabaseProviderFactory
        + HeaderProvider<Header = N::BlockHeader>
        + Clone
        + 'static,
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
        let preparation_started = Instant::now();
        let parent = ctx.parent_header();
        let retired = ctx.state_provider_factory.database_provider_ro()?.mpt_retired_at()?;
        self.validate_retired_parent(parent.header(), retired)?;
        let active = ctx.env().evm_env.block_env().timestamp.saturating_to::<u64>() >=
            self.activation_timestamp;
        if !active &&
            self.shadow_replay.is_some() &&
            (ctx.env().decoded_bal.is_some() ||
                !self.store.contains_block(parent.hash()).map_err(ProviderError::other)?)
        {
            // The MPT alone determines validity before the fork. Without a staged shadow
            // checkpoint, ordinary validation continues until a migration anchor is available.
            return <DefaultStateRootStrategy as StateRootStrategy<N, P, Evm>>::prepare(
                self.default.as_ref(),
                ctx,
            )
        }
        if parent.timestamp() < self.activation_timestamp {
            self.ensure_shadow_checkpoint(
                parent.hash(),
                parent.header(),
                ctx.state_provider_factory.database_factory(),
                ctx.overlay_manager,
            )?;
        }
        let (state, mpt_parent) =
            if retired.is_none() && parent.timestamp() >= self.activation_timestamp {
                let (state, shadow) = self
                    .store
                    .execution_parent_with_shadow(parent.hash(), parent.state_root())
                    .map_err(ProviderError::other)?;
                (state, Some(shadow))
            } else {
                let state = self.execution_parent(
                    parent.hash(),
                    parent.state_root(),
                    parent.timestamp(),
                    active,
                )?;
                (state, retired.is_none().then(|| parent.state_root()))
            };
        let mut job = PbtStateRootJob {
            state_provider_factory: ctx.state_provider_factory.clone(),
            mpt: None,
            parent: state,
            store: self.store.clone(),
            active,
            parallel: self.parallel,
            preparation_elapsed: Duration::ZERO,
        };
        if let Some(root) = mpt_parent {
            let prepared = <DefaultStateRootStrategy as StateRootStrategy<N, P, Evm>>::prepare(
                self.default.as_ref(),
                if active { ctx.with_parent_state_root(root) } else { ctx },
            )?;
            job.mpt = Some(prepared.job);
            job.preparation_elapsed = preparation_started.elapsed();
            Ok(PreparedStateRootJob { job: Box::new(job), ..prepared })
        } else {
            job.preparation_elapsed = preparation_started.elapsed();
            Ok(PreparedStateRootJob::new(Box::new(job), None))
        }
    }

    fn prepare_payload_builder(
        &self,
        ctx: PayloadStateRootJobContext<'_, N, P>,
    ) -> ProviderResult<Option<PayloadStateRootHandle>> {
        let preparation_started = Instant::now();
        let retired = ctx
            .state_provider_factory
            .database_provider_ro()
            .and_then(|provider| provider.mpt_retired_at());
        if ctx.timestamp() < self.activation_timestamp && matches!(&retired, Ok(None)) {
            return <DefaultStateRootStrategy as StateRootStrategy<N, P, Evm>>::prepare_payload_builder(self.default.as_ref(), ctx);
        }
        // Retain preparation failures in the output backend so a missing checkpoint
        // cannot select an MPT root when the payload is completed.
        let parent_number = ctx.parent_header().number();
        let timestamp = ctx.timestamp();
        let parallel = self.parallel;
        let parent = retired
            .and_then(|retired| {
                self.validate_retired_parent(ctx.parent_header(), retired)?;
                if ctx.parent_header().timestamp() < self.activation_timestamp {
                    self.ensure_shadow_checkpoint(
                        ctx.parent_hash(),
                        ctx.parent_header(),
                        ctx.state_provider_factory.database_factory(),
                        ctx.overlay_manager,
                    )?;
                }
                if retired.is_none() && ctx.parent_header().timestamp() >= self.activation_timestamp
                {
                    let (parent, shadow) = self
                        .store
                        .execution_parent_with_shadow(ctx.parent_hash(), ctx.parent_state_root())
                        .map_err(ProviderError::other)?;
                    Ok((parent, Some(shadow)))
                } else {
                    let parent = self.execution_parent(
                        ctx.parent_hash(),
                        ctx.parent_state_root(),
                        ctx.parent_header().timestamp(),
                        timestamp >= self.activation_timestamp,
                    )?;
                    Ok((parent, retired.is_none().then(|| ctx.parent_state_root())))
                }
            })
            .map_err(Arc::new);
        let store = self.store.clone();
        let parent_header = SealedHeader::new(ctx.parent_header().clone(), ctx.parent_hash());
        let executor = ctx.executor.clone();
        let config = ctx.config.clone();
        let default = self.default.clone();
        let factory = ctx.state_provider_factory;
        let preparation_elapsed = preparation_started.elapsed();
        Ok(Some(PayloadStateRootHandle::from_output("pbt", move |bundle| {
            let started = Instant::now();
            let (parent, mpt_parent) = parent
                .as_ref()
                .map_err(|error| StateRootTaskError::Other(Box::new(error.clone())))?;
            let provider = factory.database_provider_ro()?;
            let retired = provider.mpt_retired_at()?;
            if retired.is_some_and(|number| parent_number < number) {
                return Err(StateRootTaskError::Other(
                    std::io::Error::other("cannot unwind past MPT retirement; re-anchor or resync")
                        .into(),
                ))
            }
            let hashed_state = provider.hashed_post_state(bundle)?;
            drop(provider);
            let mpt_started = Instant::now();
            // Each rebuild gets its own task. Auxiliary tries must not be published under
            // the payload's PBT header root in the shared MPT sparse-trie cache.
            let mut mpt = if retired.is_none() && config.use_state_root_task() {
                let (mut handle, pending_trie) = default.spawn_state_root_with_parent_root(
                    &executor,
                    factory.clone(),
                    StateRootTaskOptions {
                        parent_header: parent_header.clone(),
                        preserved_sparse_trie: None,
                        transaction_count: None,
                        gas_used: None,
                        config: &config,
                        pending_sparse_trie_prune_blocks: None,
                    },
                    *mpt_parent,
                );
                drop(pending_trie);
                let updates = handle.take_hashed_update_stream();
                updates.on_hashed_state_update(hashed_state.clone());
                updates.finish();
                Some(handle)
            } else {
                None
            };
            let update_started = Instant::now();
            let state = parent
                .apply_bundle(bundle, parallel)
                .map_err(|err| StateRootTaskError::Other(err.into()))?;
            let update_elapsed = update_started.elapsed();
            // Retirement may have committed while PBT hashing was in progress.
            let retired = factory.database_provider_ro()?.mpt_retired_at()?;
            if retired.is_some_and(|number| parent_number < number) {
                return Err(StateRootTaskError::Other(
                    std::io::Error::other("cannot unwind past MPT retirement; re-anchor or resync")
                        .into(),
                ))
            }
            let mpt_wait_started = Instant::now();
            let trie_updates = if retired.is_some() {
                Arc::new(TrieUpdates::default())
            } else {
                let (mpt_root, updates) = if let Some(handle) = &mut mpt {
                    let rx = handle.take_state_root_rx();
                    let outcome = loop {
                        if executor.on_shutdown_signal().clone().now_or_never().is_some() {
                            return Err(StateRootTaskError::Canceled)
                        }
                        match rx.recv_timeout(Duration::from_millis(100)) {
                            Ok(result) => break result?,
                            Err(RecvTimeoutError::Timeout) => continue,
                            Err(RecvTimeoutError::Disconnected) => {
                                return Err(StateRootTaskError::Other(
                                    "sparse trie task dropped".into(),
                                ))
                            }
                        }
                    };
                    (outcome.state_root, outcome.trie_updates)
                } else {
                    let (root, updates) = factory
                        .database_provider_ro()?
                        .state_root_with_updates(hashed_state.clone())?;
                    (root, Arc::new(updates))
                };
                store
                    .cache_shadow_mpt_root(state.root(), mpt_root)
                    .map_err(|err| StateRootTaskError::Other(err.into()))?;
                updates
            };
            let mpt_ms =
                if retired.is_none() { mpt_started.elapsed().as_secs_f64() * 1000.0 } else { 0.0 };
            let mpt_wait_ms = if retired.is_none() {
                mpt_wait_started.elapsed().as_secs_f64() * 1000.0
            } else {
                0.0
            };
            let checkpoint_started = Instant::now();
            store
                .cache_execution_candidate(&state)
                .map_err(|err| StateRootTaskError::Other(err.into()))?;
            let checkpoint_elapsed = checkpoint_started.elapsed();
            let finish_elapsed = started.elapsed();
            info!(
                target: "engine::pbt",
                phase = "payload",
                parent_number,
                timestamp,
                parallel_requested = parallel,
                parallel = state.uses_parallel_batch(parallel),
                disk_overlay = matches!(&state, ExecutionState::Disk(_)),
                maintain_mpt = retired.is_none(),
                accounts = bundle.state().len(),
                leaves = state.leaf_count(),
                pbt_root = ?state.root(),
                pbt_update_ms = update_elapsed.as_secs_f64() * 1000.0,
                mpt_ms,
                mpt_wait_ms,
                checkpoint_ms = 0.0,
                candidate_cache_ms = checkpoint_elapsed.as_secs_f64() * 1000.0,
                prepare_ms = preparation_elapsed.as_secs_f64() * 1000.0,
                finish_ms = finish_elapsed.as_secs_f64() * 1000.0,
                total_ms = (preparation_elapsed + finish_elapsed).as_secs_f64() * 1000.0,
                "PBT state root timings"
            );
            Ok(StateRootComputeOutcome {
                state_root: state.root(),
                trie_updates,
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
    parent: ExecutionParent<H>,
    store: Arc<SnapshotStore<H>>,
    active: bool,
    parallel: bool,
    preparation_elapsed: Duration,
}

impl<N, P, H> StateRootJob<N> for PbtStateRootJob<N, P, H>
where
    N: NodePrimitives,
    P: BalProvider
        + DatabaseProviderFactory
        + HeaderProvider<Header = N::BlockHeader>
        + Clone
        + 'static,
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
        let state =
            self.parent.apply_bundle(&output.state, self.parallel).map_err(ProviderError::other)?;
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
                .save_execution_block(block.hash(), block.number(), &state)
                .map_err(ProviderError::other)?;
        }
        let checkpoint_ms =
            if checkpointed { checkpoint_started.elapsed().as_secs_f64() * 1000.0 } else { 0.0 };
        let finish_elapsed = started.elapsed();
        info!(
            target: "engine::pbt",
            phase = "validation",
            block_number = block.number(),
            block_hash = ?block.hash(),
            pbt_root = ?state.root(),
            parallel_requested = self.parallel,
            parallel = state.uses_parallel_batch(self.parallel),
            disk_overlay = matches!(&state, ExecutionState::Disk(_)),
            maintain_mpt,
            active = self.active,
            accounts,
            leaves = state.leaf_count(),
            pbt_update_ms = update_elapsed.as_secs_f64() * 1000.0,
            mpt_ms,
            checkpoint_ms,
            checkpointed,
            prepare_ms = self.preparation_elapsed.as_secs_f64() * 1000.0,
            finish_ms = finish_elapsed.as_secs_f64() * 1000.0,
            total_ms = (self.preparation_elapsed + finish_elapsed).as_secs_f64() * 1000.0,
            "PBT state root timings"
        );
        Ok(outcome)
    }
}
