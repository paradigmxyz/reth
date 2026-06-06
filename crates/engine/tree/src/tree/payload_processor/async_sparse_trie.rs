//! Experimental Tokio actor based sparse trie task for BAL state updates.

use crate::tree::payload_processor::{
    multiproof::{
        dispatch_with_chunking, MultiProofTaskMetrics, StateRootComputeOutcome, StateRootMessage,
        DEFAULT_MAX_TARGETS_FOR_CHUNKING,
    },
    preserved_sparse_trie::{PreservedSparseTrie, SharedPreservedSparseTrie},
    SPARSE_TRIE_MAX_NODES_SHRINK_CAPACITY, SPARSE_TRIE_MAX_VALUES_SHRINK_CAPACITY,
};
use alloy_primitives::{
    map::{B256Map, B256Set, Entry},
    B256,
};
use alloy_rlp::{Decodable, Encodable};
use crossbeam_channel::{
    Receiver as CrossbeamReceiver, RecvTimeoutError, Sender as CrossbeamSender,
};
use reth_primitives_traits::{Account, FastInstant as Instant};
use reth_tasks::Runtime;
use reth_trie::{
    updates::TrieUpdates, DecodedMultiProofV2, HashedPostState, HashedStorage, MultiProofTargetsV2,
    Nibbles, ProofTrieNodeV2, ProofV2Target, TrieAccount, EMPTY_ROOT_HASH,
    TRIE_ACCOUNT_RLP_MAX_SIZE,
};
use reth_trie_parallel::{
    proof_task::{
        AccountMultiproofInput, ProofResultContext, ProofResultMessage, ProofWorkerHandle,
    },
    root::ParallelStateRootError,
};
use reth_trie_sparse::{
    errors::{SparseStateTrieErrorKind, SparseTrieErrorKind},
    ArenaParallelSparseTrie, ConfigurableSparseTrie, DeferredDrops, LeafUpdate,
    RevealableSparseTrie, SparseStateTrie, SparseTrie,
};
use std::{
    future::Future,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc, Arc,
    },
    thread::{self, JoinHandle as StdJoinHandle},
    time::Duration,
};
use tokio::{
    runtime::{Builder, Runtime as TokioRuntime},
    sync::{
        mpsc::{error::TryRecvError, unbounded_channel, UnboundedReceiver, UnboundedSender},
        oneshot,
    },
};
use tracing::{debug, debug_span, error, instrument, trace_span, Instrument};

/// Environment variable that disables the async BAL sparse trie prototype when set to a falsy
/// value.
const ASYNC_BAL_SPARSE_TRIE_ENV: &str = "RETH_ASYNC_BAL_SPARSE_TRIE";
/// Environment variable that controls async sparse trie runtime worker count.
const ASYNC_SPARSE_TRIE_THREADS_ENV: &str = "RETH_ASYNC_SPARSE_TRIE_THREADS";
/// Default worker count for the persistent async sparse trie runtime.
const DEFAULT_ASYNC_SPARSE_TRIE_THREADS: usize = 32;
/// Prefix for persistent async sparse trie runtime worker thread names.
const ASYNC_SPARSE_TRIE_THREAD_PREFIX: &str = "async-sr";
/// Maximum account updates an actor applies in one `update_leaves` call.
const MAX_ACCOUNT_UPDATES_PER_DRIVE: usize = 1024;
/// Maximum storage updates an actor applies in one `update_leaves` call.
const MAX_STORAGE_UPDATES_PER_DRIVE: usize = 1024;
/// Poll interval used by bridge threads so they can be stopped without waiting on channel close.
const BRIDGE_RECV_TIMEOUT: Duration = Duration::from_millis(10);

type StateTrie = SparseStateTrie<ConfigurableSparseTrie, ConfigurableSparseTrie>;
type ActorTrie = RevealableSparseTrie<ConfigurableSparseTrie>;

/// Persistent Tokio runtime for the async BAL sparse trie prototype.
pub(super) struct AsyncSparseTrieRuntime {
    runtime: TokioRuntime,
}

impl std::fmt::Debug for AsyncSparseTrieRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AsyncSparseTrieRuntime").finish_non_exhaustive()
    }
}

impl AsyncSparseTrieRuntime {
    /// Builds a new persistent async sparse trie runtime.
    pub(super) fn new() -> Result<Arc<Self>, String> {
        let threads = async_sparse_trie_threads();
        let next_thread_id = Arc::new(AtomicUsize::new(0));
        let thread_id = next_thread_id.clone();

        let runtime = Builder::new_multi_thread()
            .worker_threads(threads)
            .thread_name_fn(move || {
                let id = thread_id.fetch_add(1, Ordering::Relaxed);
                format!("{ASYNC_SPARSE_TRIE_THREAD_PREFIX}-{id}")
            })
            .on_thread_start(reth_tasks::utils::increase_thread_priority)
            .enable_all()
            .build()
            .map_err(|error| format!("failed to build async sparse trie runtime: {error}"))?;

        Ok(Arc::new(Self { runtime }))
    }

    /// Spawns a future on the persistent async sparse trie runtime.
    pub(super) fn spawn<F>(&self, future: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.runtime.spawn(future);
    }
}

fn async_sparse_trie_threads() -> usize {
    std::env::var(ASYNC_SPARSE_TRIE_THREADS_ENV)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|threads| *threads > 0)
        .unwrap_or(DEFAULT_ASYNC_SPARSE_TRIE_THREADS)
}

/// Returns whether the experimental async BAL sparse trie path should be selected.
pub(super) fn async_bal_sparse_trie_enabled() -> bool {
    std::env::var(ASYNC_BAL_SPARSE_TRIE_ENV).map_or(true, |value| {
        !matches!(value.as_str(), "0" | "false" | "FALSE" | "no" | "NO" | "off" | "OFF")
    })
}

/// Runs the experimental async BAL sparse trie task.
#[instrument(
    name = "async_sr_task",
    level = "debug",
    target = "engine::tree::payload_processor::async_sparse_trie",
    skip_all
)]
#[expect(clippy::too_many_arguments)]
pub(super) async fn run_async_bal_sparse_trie_task(
    preserved_sparse_trie: SharedPreservedSparseTrie,
    executor: Runtime,
    proof_worker_handle: ProofWorkerHandle,
    trie_metrics: MultiProofTaskMetrics,
    state_root_tx: mpsc::Sender<Result<StateRootComputeOutcome, ParallelStateRootError>>,
    final_hashed_state_tx: mpsc::Sender<HashedPostState>,
    from_multi_proof: CrossbeamReceiver<StateRootMessage>,
    parent_state_root: B256,
    chunk_size: usize,
    max_hot_slots: usize,
    max_hot_accounts: usize,
    disable_cache_pruning: bool,
) {
    // TODO: accumulate and forward final hashed state from the async BAL coordinator. Until then,
    // dropping the sender makes validators fall back to their existing recomputation path.
    drop(final_hashed_state_tx);

    let start = Instant::now();
    let preserved = preserved_sparse_trie.take();
    trie_metrics.sparse_trie_cache_wait_duration_histogram.record(start.elapsed().as_secs_f64());

    let mut sparse_state_trie = preserved
        .map(|preserved| preserved.into_trie_for(parent_state_root))
        .unwrap_or_else(|| {
            debug!(
                target: "engine::tree::payload_processor",
                "Creating new async BAL sparse trie - no preserved trie available"
            );
            let default_trie = RevealableSparseTrie::blind_from(ConfigurableSparseTrie::Arena(
                ArenaParallelSparseTrie::default(),
            ));
            SparseStateTrie::default()
                .with_accounts_trie(default_trie.clone())
                .with_default_storage_trie(default_trie)
                .with_updates(true)
        });
    sparse_state_trie.set_hot_cache_capacities(max_hot_slots, max_hot_accounts);

    let coordinator_metrics = trie_metrics.clone();
    let output = AsyncSparseTrieCoordinator::new(
        sparse_state_trie,
        from_multi_proof,
        proof_worker_handle,
        coordinator_metrics,
        parent_state_root,
        chunk_size,
    )
    .run()
    .await;

    preserve_async_sparse_trie(
        preserved_sparse_trie,
        executor,
        trie_metrics,
        state_root_tx,
        output,
        max_hot_slots,
        max_hot_accounts,
        disable_cache_pruning,
    );
}

#[expect(clippy::too_many_arguments)]
fn preserve_async_sparse_trie(
    preserved_sparse_trie: SharedPreservedSparseTrie,
    executor: Runtime,
    trie_metrics: MultiProofTaskMetrics,
    state_root_tx: mpsc::Sender<Result<StateRootComputeOutcome, ParallelStateRootError>>,
    mut output: CoordinatorOutput,
    max_hot_slots: usize,
    max_hot_accounts: usize,
    disable_cache_pruning: bool,
) {
    let _span = debug_span!(
        target: "engine::tree::payload_processor::async_sparse_trie",
        "async_sr_preserve_trie"
    )
    .entered();

    let mut guard = preserved_sparse_trie.lock();
    let task_result = output.result.as_ref().ok().cloned();

    if state_root_tx.send(output.result).is_err() {
        debug!(
            target: "engine::tree::payload_processor",
            "Async sparse trie state root receiver dropped, clearing trie"
        );
        output.trie.clear();
        output.trie.shrink_to(
            SPARSE_TRIE_MAX_NODES_SHRINK_CAPACITY,
            SPARSE_TRIE_MAX_VALUES_SHRINK_CAPACITY,
        );
        guard.store(PreservedSparseTrie::cleared(output.trie));
        drop(guard);
        executor.spawn_drop(output.deferred);
        return;
    }

    let deferred = if let Some(result) = task_result {
        let _enter =
            debug_span!(target: "engine::tree::payload_processor", "async_preserve").entered();
        let start = Instant::now();
        if !disable_cache_pruning {
            output.trie.commit_updates(&result.trie_updates);
            output.trie.prune(max_hot_slots, max_hot_accounts);
        }
        output.trie.shrink_to(
            SPARSE_TRIE_MAX_NODES_SHRINK_CAPACITY,
            SPARSE_TRIE_MAX_VALUES_SHRINK_CAPACITY,
        );
        trie_metrics.into_trie_for_reuse_duration_histogram.record(start.elapsed().as_secs_f64());
        trie_metrics.sparse_trie_retained_memory_bytes.set(output.trie.memory_size() as f64);
        trie_metrics
            .sparse_trie_retained_storage_tries
            .set(output.trie.retained_storage_tries_count() as f64);
        guard.store(PreservedSparseTrie::anchored(output.trie, result.state_root));
        output.deferred
    } else {
        debug!(
            target: "engine::tree::payload_processor",
            "Async sparse trie state root computation failed, clearing trie"
        );
        output.trie.clear();
        output.trie.shrink_to(
            SPARSE_TRIE_MAX_NODES_SHRINK_CAPACITY,
            SPARSE_TRIE_MAX_VALUES_SHRINK_CAPACITY,
        );
        guard.store(PreservedSparseTrie::cleared(output.trie));
        output.deferred
    };
    drop(guard);
    executor.spawn_drop(deferred);
}

struct CoordinatorOutput {
    result: Result<StateRootComputeOutcome, ParallelStateRootError>,
    trie: StateTrie,
    deferred: DeferredDrops,
}

struct AsyncSparseTrieCoordinator {
    trie: StateTrie,
    parent_state_root: B256,
    state_rx: UnboundedReceiver<StateRootMessage>,
    state_bridge: Option<BridgeHandle>,
    event_tx: UnboundedSender<CoordinatorEvent>,
    event_rx: UnboundedReceiver<CoordinatorEvent>,
    account_tx: UnboundedSender<AccountTrieCommand>,
    proof_tx: Option<UnboundedSender<ProofServiceCommand>>,
    proof_bridge: Option<BridgeHandle>,
    storage_actors: B256Map<StorageActorHandle>,
    pending_storage_roots: B256Set,
    storage_roots: B256Map<B256>,
    pending_accounts: B256Map<PendingAccountUpdate>,
    finished_state_updates: bool,
    finalize_sent: bool,
    metrics: MultiProofTaskMetrics,
    started_at: Instant,
}

impl AsyncSparseTrieCoordinator {
    fn new(
        mut trie: StateTrie,
        from_multi_proof: CrossbeamReceiver<StateRootMessage>,
        proof_worker_handle: ProofWorkerHandle,
        metrics: MultiProofTaskMetrics,
        parent_state_root: B256,
        chunk_size: usize,
    ) -> Self {
        let (state_tx, state_rx) = unbounded_channel();
        let state_bridge = spawn_state_message_bridge(from_multi_proof, state_tx);

        let (event_tx, event_rx) = unbounded_channel();
        let (account_tx, account_rx) = unbounded_channel();

        let (proof_tx, proof_rx) = unbounded_channel();
        let (proof_result_tx, proof_result_rx) = crossbeam_channel::unbounded();
        let (proof_event_tx, proof_event_rx) = unbounded_channel();
        let proof_bridge = spawn_proof_result_bridge(proof_result_rx, proof_event_tx);

        let account_trie = trie.take_accounts_trie();
        let account_actor =
            AccountTrieActor::new(account_trie, account_rx, event_tx.clone(), proof_tx.clone());
        tokio::spawn(account_actor.run().instrument(debug_span!(
            target: "engine::tree::payload_processor::async_sparse_trie",
            "async_sr_account_actor"
        )));

        let proof_service = ProofService::new(
            proof_worker_handle,
            proof_result_tx,
            proof_rx,
            proof_event_rx,
            account_tx.clone(),
            event_tx.clone(),
            chunk_size,
        );
        tokio::spawn(proof_service.run().instrument(debug_span!(
            target: "engine::tree::payload_processor::async_sparse_trie",
            "async_sr_proof_service"
        )));

        Self {
            trie,
            parent_state_root,
            state_rx,
            state_bridge: Some(state_bridge),
            event_tx,
            event_rx,
            account_tx,
            proof_tx: Some(proof_tx),
            proof_bridge: Some(proof_bridge),
            storage_actors: B256Map::default(),
            pending_storage_roots: B256Set::default(),
            storage_roots: B256Map::default(),
            pending_accounts: B256Map::default(),
            finished_state_updates: false,
            finalize_sent: false,
            metrics,
            started_at: Instant::now(),
        }
    }

    #[instrument(
        name = "async_sr_coordinator",
        level = "debug",
        target = "engine::tree::payload_processor::async_sparse_trie",
        skip_all
    )]
    async fn run(mut self) -> CoordinatorOutput {
        let mut progress_interval = tokio::time::interval(Duration::from_secs(1));

        loop {
            tokio::select! {
                maybe_message = self.state_rx.recv(), if !self.finished_state_updates => {
                    let Some(message) = maybe_message else {
                        return self.finish_with_error(ParallelStateRootError::Other(
                            "async sparse trie update stream closed before FinishedStateUpdates".to_string(),
                        )).await;
                    };

                    if let Err(error) = self.on_state_root_message(message).await {
                        return self.finish_with_error(error).await;
                    }
                }
                maybe_event = self.event_rx.recv() => {
                    let Some(event) = maybe_event else {
                        return self.finish_with_error(ParallelStateRootError::Other(
                            "async sparse trie actor event stream closed".to_string(),
                        )).await;
                    };

                    match event {
                        CoordinatorEvent::StorageRootReady { address, root } => {
                            self.storage_roots.insert(address, root);
                            if let Err(error) = self.maybe_finalize_accounts() {
                                return self.finish_with_error(error).await;
                            }
                        }
                        CoordinatorEvent::AccountFinalized => {
                            return self.finish_success().await;
                        }
                        CoordinatorEvent::Error(error) => {
                            return self.finish_with_error(error).await;
                        }
                    }
                }
                _ = progress_interval.tick(), if self.finished_state_updates => {
                    self.log_progress();
                }
            }
        }
    }

    fn log_progress(&self) {
        let missing_storage_roots = self
            .pending_storage_roots
            .iter()
            .filter(|address| !self.storage_roots.contains_key(*address))
            .count();
        debug!(
            target: "engine::tree::payload_processor::async_sparse_trie",
            storage_actors = self.storage_actors.len(),
            pending_storage_roots = self.pending_storage_roots.len(),
            ready_storage_roots = self.storage_roots.len(),
            missing_storage_roots,
            pending_accounts = self.pending_accounts.len(),
            finalize_sent = self.finalize_sent,
            "async sparse trie coordinator progress"
        );
    }

    async fn on_state_root_message(
        &mut self,
        message: StateRootMessage,
    ) -> Result<(), ParallelStateRootError> {
        match message {
            StateRootMessage::HashedStateUpdate(update) => {
                self.on_hashed_state_update(update).await?;
            }
            StateRootMessage::PrefetchProofs(targets) => {
                self.on_prewarm_targets(targets).await?;
            }
            StateRootMessage::FinishedStateUpdates => {
                let _span = debug_span!(
                    target: "engine::tree::payload_processor::async_sparse_trie",
                    "async_sr_finished_state_updates",
                    storage_actors = self.storage_actors.len(),
                    pending_storage_roots = self.pending_storage_roots.len(),
                    pending_accounts = self.pending_accounts.len(),
                )
                .entered();
                self.finished_state_updates = true;
                self.drive_all_actors()?;
                self.maybe_finalize_accounts()?;
            }
            StateRootMessage::StateUpdate(_, _) | StateRootMessage::BlockAccessList(_) => {
                return Err(ParallelStateRootError::Other(
                    "async BAL sparse trie prototype only accepts pre-hashed BAL updates"
                        .to_string(),
                ));
            }
        }

        Ok(())
    }

    async fn on_hashed_state_update(
        &mut self,
        hashed_state_update: HashedPostState,
    ) -> Result<(), ParallelStateRootError> {
        let accounts_len = hashed_state_update.accounts.len();
        let storages_len = hashed_state_update.storages.len();
        let storage_slots = hashed_state_update
            .storages
            .values()
            .map(|storage| storage.storage.len())
            .sum::<usize>();
        let _span = trace_span!(
            target: "engine::tree::payload_processor::async_sparse_trie",
            "async_sr_hashed_state_update",
            accounts = accounts_len,
            storages = storages_len,
            storage_slots,
        )
        .entered();

        match (accounts_len, storages_len) {
            (0, 0) => Ok(()),
            (1, 0) => {
                let (address, account) =
                    hashed_state_update.accounts.into_iter().next().expect("checked len");
                self.trie.record_account_touch(address);
                self.pending_accounts.insert(address, PendingAccountUpdate::AccountChanged(account));
                self.send_account_command(AccountTrieCommand::Touch(address))?;
                Ok(())
            }
            (0, 1) => {
                let (address, storage) =
                    hashed_state_update.storages.into_iter().next().expect("checked len");
                if storage.wiped {
                    return Err(ParallelStateRootError::Other(
                        "async BAL sparse trie prototype does not support storage wipes yet"
                            .to_string(),
                    ));
                }

                for &slot in storage.storage.keys() {
                    self.trie.record_slot_touch(address, slot);
                }

                if !storage.storage.is_empty() {
                    self.pending_accounts
                        .entry(address)
                        .or_insert(PendingAccountUpdate::StorageOnly);
                    self.pending_storage_roots.insert(address);

                    let storage_tx = self.ensure_storage_actor(address)?;
                    send_storage_command(
                        &storage_tx,
                        StorageTrieCommand::ApplyHashedStorage(storage),
                    )?;
                    self.send_account_command(AccountTrieCommand::Touch(address))?;
                }

                Ok(())
            }
            _ => Err(ParallelStateRootError::Other(format!(
                "async BAL sparse trie expected one-account-scoped hashed update, got {accounts_len} account entries and {storages_len} storage entries",
            ))),
        }
    }

    async fn on_prewarm_targets(
        &mut self,
        targets: MultiProofTargetsV2,
    ) -> Result<(), ParallelStateRootError> {
        let account_targets = targets.account_targets.len();
        let storage_tries = targets.storage_targets.len();
        let storage_targets =
            targets.storage_targets.values().map(|targets| targets.len()).sum::<usize>();
        let _span = debug_span!(
            target: "engine::tree::payload_processor::async_sparse_trie",
            "async_sr_prewarm_targets",
            account_targets,
            storage_tries,
            storage_targets,
        )
        .entered();

        for target in targets.account_targets {
            self.send_account_command(AccountTrieCommand::Touch(target.key()))?;
        }

        for (address, slots) in targets.storage_targets {
            if !slots.is_empty() {
                let storage_tx = self.ensure_storage_actor(address)?;
                let slots = slots.into_iter().map(|target| target.key()).collect();
                send_storage_command(&storage_tx, StorageTrieCommand::TouchSlots(slots))?;
                send_storage_command(&storage_tx, StorageTrieCommand::Drive)?;
            }
            self.send_account_command(AccountTrieCommand::Touch(address))?;
        }
        self.send_account_command(AccountTrieCommand::Drive)?;

        Ok(())
    }

    fn ensure_storage_actor(
        &mut self,
        address: B256,
    ) -> Result<UnboundedSender<StorageTrieCommand>, ParallelStateRootError> {
        if let Some(actor) = self.storage_actors.get(&address) {
            return Ok(actor.tx.clone())
        }

        let trie = self.trie.take_or_create_storage_trie(&address);
        let (tx, rx) = unbounded_channel();
        let actor =
            StorageTrieActor::new(address, trie, rx, self.event_tx.clone(), self.proof_tx()?);
        tokio::spawn(actor.run().instrument(debug_span!(
            target: "engine::tree::payload_processor::async_sparse_trie",
            "async_sr_storage_actor",
            ?address,
        )));

        self.send_proof_command(ProofServiceCommand::RegisterStorageActor {
            address,
            tx: tx.clone(),
        })?;
        self.storage_actors.insert(address, StorageActorHandle { tx: tx.clone() });

        Ok(tx)
    }

    fn drive_all_actors(&self) -> Result<(), ParallelStateRootError> {
        let _span = debug_span!(
            target: "engine::tree::payload_processor::async_sparse_trie",
            "async_sr_drive_all_actors",
            storage_actors = self.storage_actors.len(),
            pending_storage_roots = self.pending_storage_roots.len(),
            pending_accounts = self.pending_accounts.len(),
        )
        .entered();

        self.send_account_command(AccountTrieCommand::Drive)?;
        for actor in self.storage_actors.values() {
            send_storage_command(&actor.tx, StorageTrieCommand::Drive)?;
        }
        Ok(())
    }

    fn maybe_finalize_accounts(&mut self) -> Result<(), ParallelStateRootError> {
        if !self.finished_state_updates || self.finalize_sent {
            return Ok(())
        }

        if self
            .pending_storage_roots
            .iter()
            .any(|address| !self.storage_roots.contains_key(address))
        {
            return Ok(())
        }

        let pending_accounts = std::mem::take(&mut self.pending_accounts);
        let _span = debug_span!(
            target: "engine::tree::payload_processor::async_sparse_trie",
            "async_sr_send_account_finalize",
            pending_accounts = pending_accounts.len(),
            storage_roots = self.storage_roots.len(),
        )
        .entered();
        self.send_account_command(AccountTrieCommand::FinalizeAccounts {
            pending_accounts,
            storage_roots: self.storage_roots.clone(),
        })?;
        self.finalize_sent = true;

        Ok(())
    }

    async fn finish_success(mut self) -> CoordinatorOutput {
        let storage_actors = self.storage_actors.len();
        let actor_result = self
            .return_actor_tries()
            .instrument(debug_span!(
                target: "engine::tree::payload_processor::async_sparse_trie",
                "async_sr_return_actor_tries",
                storage_actors,
            ))
            .await;
        self.shutdown_bridges();

        let result = actor_result.and_then(|_| self.compute_outcome());
        let deferred = self.trie.take_deferred_drops();

        CoordinatorOutput { result, trie: self.trie, deferred }
    }

    async fn finish_with_error(mut self, error: ParallelStateRootError) -> CoordinatorOutput {
        let storage_actors = self.storage_actors.len();
        let actor_result = self
            .return_actor_tries()
            .instrument(debug_span!(
                target: "engine::tree::payload_processor::async_sparse_trie",
                "async_sr_return_actor_tries",
                storage_actors,
            ))
            .await;
        self.shutdown_bridges();

        let result = actor_result.and(Err(error));
        let deferred = self.trie.take_deferred_drops();

        CoordinatorOutput { result, trie: self.trie, deferred }
    }

    async fn return_actor_tries(&mut self) -> Result<(), ParallelStateRootError> {
        let (account_return_tx, account_return_rx) = oneshot::channel();
        self.send_account_command(AccountTrieCommand::ReturnTrie { tx: account_return_tx })?;
        let (account_trie, account_deferred) = account_return_rx.await.map_err(|_| {
            ParallelStateRootError::Other(
                "async account sparse trie actor dropped before returning trie".to_string(),
            )
        })?;
        self.trie.insert_accounts_trie(account_trie);
        self.trie.extend_deferred_drops(account_deferred);

        for (address, actor) in std::mem::take(&mut self.storage_actors) {
            let (return_tx, return_rx) = oneshot::channel();
            send_storage_command(&actor.tx, StorageTrieCommand::ReturnTrie { tx: return_tx })?;
            let (storage_trie, storage_deferred) = return_rx.await.map_err(|_| {
                ParallelStateRootError::Other(format!(
                    "async storage sparse trie actor for {address:?} dropped before returning trie"
                ))
            })?;
            self.trie.insert_storage_trie(address, storage_trie);
            self.trie.extend_deferred_drops(storage_deferred);
        }

        self.proof_tx.take();

        Ok(())
    }

    fn compute_outcome(&mut self) -> Result<StateRootComputeOutcome, ParallelStateRootError> {
        let _span = debug_span!(
            target: "engine::tree::payload_processor::async_sparse_trie",
            "async_sr_compute_outcome"
        )
        .entered();
        let start = Instant::now();
        let (state_root, trie_updates) = match self.trie.root_with_updates() {
            Ok(result) => result,
            Err(err)
                if matches!(
                    err.kind(),
                    SparseStateTrieErrorKind::Sparse(SparseTrieErrorKind::Blind)
                ) =>
            {
                (self.parent_state_root, TrieUpdates::default())
            }
            Err(err) => {
                return Err(ParallelStateRootError::Other(format!(
                    "could not calculate async sparse trie state root: {err:?}"
                )))
            }
        };

        #[cfg(feature = "trie-debug")]
        let debug_recorders = self.trie.take_debug_recorders();

        let end = Instant::now();
        self.metrics.sparse_trie_final_update_duration_histogram.record(end.duration_since(start));
        self.metrics
            .sparse_trie_total_duration_histogram
            .record(end.duration_since(self.started_at));

        Ok(StateRootComputeOutcome {
            state_root,
            trie_updates: Arc::new(trie_updates),
            #[cfg(feature = "trie-debug")]
            debug_recorders,
        })
    }

    fn shutdown_bridges(&mut self) {
        self.proof_tx.take();
        if let Some(bridge) = self.state_bridge.take() {
            bridge.stop();
        }
        if let Some(bridge) = self.proof_bridge.take() {
            bridge.stop();
        }
    }

    fn proof_tx(&self) -> Result<UnboundedSender<ProofServiceCommand>, ParallelStateRootError> {
        self.proof_tx.clone().ok_or_else(|| {
            ParallelStateRootError::Other("async sparse trie proof service stopped".to_string())
        })
    }

    fn send_proof_command(
        &self,
        command: ProofServiceCommand,
    ) -> Result<(), ParallelStateRootError> {
        self.proof_tx()?.send(command).map_err(|_| {
            ParallelStateRootError::Other("async sparse trie proof service dropped".to_string())
        })
    }

    fn send_account_command(
        &self,
        command: AccountTrieCommand,
    ) -> Result<(), ParallelStateRootError> {
        self.account_tx.send(command).map_err(|_| {
            ParallelStateRootError::Other("async account sparse trie actor dropped".to_string())
        })
    }
}

struct StorageActorHandle {
    tx: UnboundedSender<StorageTrieCommand>,
}

#[derive(Debug, Clone, Copy)]
enum PendingAccountUpdate {
    StorageOnly,
    AccountChanged(Option<Account>),
}

enum CoordinatorEvent {
    StorageRootReady { address: B256, root: B256 },
    AccountFinalized,
    Error(ParallelStateRootError),
}

enum AccountTrieCommand {
    Touch(B256),
    Reveal(Vec<ProofTrieNodeV2>),
    Drive,
    FinalizeAccounts {
        pending_accounts: B256Map<PendingAccountUpdate>,
        storage_roots: B256Map<B256>,
    },
    ReturnTrie {
        tx: oneshot::Sender<(ActorTrie, DeferredDrops)>,
    },
}

struct AccountTrieActor {
    trie: ActorTrie,
    updates: B256Map<LeafUpdate>,
    finalize: Option<AccountFinalize>,
    final_updates_built: bool,
    finalized: bool,
    account_rlp_buf: Vec<u8>,
    deferred: DeferredDrops,
    rx: UnboundedReceiver<AccountTrieCommand>,
    event_tx: UnboundedSender<CoordinatorEvent>,
    proof_tx: UnboundedSender<ProofServiceCommand>,
}

impl AccountTrieActor {
    fn new(
        trie: ActorTrie,
        rx: UnboundedReceiver<AccountTrieCommand>,
        event_tx: UnboundedSender<CoordinatorEvent>,
        proof_tx: UnboundedSender<ProofServiceCommand>,
    ) -> Self {
        Self {
            trie,
            updates: B256Map::default(),
            finalize: None,
            final_updates_built: false,
            finalized: false,
            account_rlp_buf: Vec::with_capacity(TRIE_ACCOUNT_RLP_MAX_SIZE),
            deferred: DeferredDrops::default(),
            rx,
            event_tx,
            proof_tx,
        }
    }

    #[instrument(
        name = "async_sr_account_actor_run",
        level = "debug",
        target = "engine::tree::payload_processor::async_sparse_trie",
        skip_all
    )]
    async fn run(mut self) {
        while let Some(command) = self.rx.recv().await {
            if let AccountTrieCommand::ReturnTrie { tx } = command {
                let _ = tx.send((self.trie, self.deferred));
                return;
            }

            if let Err(error) = self.on_command(command).await {
                let _ = self.event_tx.send(CoordinatorEvent::Error(error));
            }
        }
    }

    async fn on_command(
        &mut self,
        command: AccountTrieCommand,
    ) -> Result<(), ParallelStateRootError> {
        let should_drive = match command {
            AccountTrieCommand::Touch(address) => {
                insert_leaf_update(&mut self.updates, address, LeafUpdate::Touched);
                false
            }
            AccountTrieCommand::Reveal(mut nodes) => {
                self.trie
                    .reveal_v2_proof_nodes(&mut nodes, true)
                    .map_err(to_parallel_sparse_error)?;
                self.deferred.proof_nodes_bufs.push(nodes);
                true
            }
            AccountTrieCommand::Drive => true,
            AccountTrieCommand::FinalizeAccounts { pending_accounts, storage_roots } => {
                self.finalize = Some(AccountFinalize { pending_accounts, storage_roots });
                true
            }
            AccountTrieCommand::ReturnTrie { .. } => unreachable!("handled by run"),
        };

        if should_drive {
            self.drive().await
        } else {
            Ok(())
        }
    }

    #[instrument(
        name = "async_sr_account_drive",
        level = "debug",
        target = "engine::tree::payload_processor::async_sparse_trie",
        skip_all,
        fields(
            updates = self.updates.len(),
            finalize = self.finalize.is_some(),
            final_updates_built = self.final_updates_built,
            finalized = self.finalized,
        )
    )]
    async fn drive(&mut self) -> Result<(), ParallelStateRootError> {
        self.try_build_final_updates()?;

        loop {
            if self.updates.is_empty() {
                self.try_build_final_updates()?;
                if self.updates.is_empty() && self.final_updates_built && !self.finalized {
                    self.finalized = true;
                    let _ = self.event_tx.send(CoordinatorEvent::AccountFinalized);
                }
                return Ok(())
            }

            let updates_len_before = self.updates.len();
            let mut chunk = drain_update_chunk(&mut self.updates, MAX_ACCOUNT_UPDATES_PER_DRIVE);
            let mut targets = Vec::new();
            self.trie
                .update_leaves(&mut chunk, |path, min_len| {
                    targets.push(ProofV2Target::new(path).with_min_len(min_len));
                })
                .map_err(to_parallel_sparse_error)?;
            merge_leaf_updates(&mut self.updates, chunk);

            if !targets.is_empty() {
                self.proof_tx.send(ProofServiceCommand::AccountTargets(targets)).map_err(|_| {
                    ParallelStateRootError::Other(
                        "async sparse trie proof service dropped".to_string(),
                    )
                })?;
                return Ok(())
            }

            if self.updates.len() == updates_len_before {
                return Err(ParallelStateRootError::Other(
                    "account sparse trie actor made no progress without proof targets".to_string(),
                ));
            }
        }
    }

    fn try_build_final_updates(&mut self) -> Result<(), ParallelStateRootError> {
        if self.final_updates_built || !self.updates.is_empty() {
            return Ok(())
        }

        let Some(AccountFinalize { pending_accounts, storage_roots }) = self.finalize.take() else {
            return Ok(())
        };
        let _span = debug_span!(
            target: "engine::tree::payload_processor::async_sparse_trie",
            "async_sr_build_account_final_updates",
            pending_accounts = pending_accounts.len(),
            storage_roots = storage_roots.len(),
        )
        .entered();

        for (address, pending) in pending_accounts {
            let trie_account = self
                .trie
                .as_revealed_ref()
                .and_then(|trie| trie.get_leaf_value(&Nibbles::unpack(address)))
                .map(|value| TrieAccount::decode(&mut &value[..]))
                .transpose()?;

            let storage_root = storage_roots
                .get(&address)
                .copied()
                .or_else(|| trie_account.as_ref().map(|account| account.storage_root))
                .unwrap_or(EMPTY_ROOT_HASH);

            let account = match pending {
                PendingAccountUpdate::StorageOnly => trie_account.map(Into::into),
                PendingAccountUpdate::AccountChanged(account) => account,
            };

            let encoded =
                encode_account_leaf_value(account, storage_root, &mut self.account_rlp_buf);
            self.updates.insert(address, LeafUpdate::Changed(encoded));
        }

        self.final_updates_built = true;

        Ok(())
    }
}

struct AccountFinalize {
    pending_accounts: B256Map<PendingAccountUpdate>,
    storage_roots: B256Map<B256>,
}

enum StorageTrieCommand {
    TouchSlots(Vec<B256>),
    ApplyHashedStorage(HashedStorage),
    Reveal(Vec<ProofTrieNodeV2>),
    Drive,
    ReturnTrie { tx: oneshot::Sender<(ActorTrie, DeferredDrops)> },
}

struct StorageTrieActor {
    address: B256,
    trie: ActorTrie,
    updates: B256Map<LeafUpdate>,
    needs_root: bool,
    root: Option<B256>,
    deferred: DeferredDrops,
    rx: UnboundedReceiver<StorageTrieCommand>,
    event_tx: UnboundedSender<CoordinatorEvent>,
    proof_tx: UnboundedSender<ProofServiceCommand>,
}

impl StorageTrieActor {
    fn new(
        address: B256,
        trie: ActorTrie,
        rx: UnboundedReceiver<StorageTrieCommand>,
        event_tx: UnboundedSender<CoordinatorEvent>,
        proof_tx: UnboundedSender<ProofServiceCommand>,
    ) -> Self {
        Self {
            address,
            trie,
            updates: B256Map::default(),
            needs_root: false,
            root: None,
            deferred: DeferredDrops::default(),
            rx,
            event_tx,
            proof_tx,
        }
    }

    #[instrument(
        name = "async_sr_storage_actor_run",
        level = "debug",
        target = "engine::tree::payload_processor::async_sparse_trie",
        skip_all,
        fields(address = ?self.address)
    )]
    async fn run(mut self) {
        while let Some(command) = self.rx.recv().await {
            if let StorageTrieCommand::ReturnTrie { tx } = command {
                let _ = tx.send((self.trie, self.deferred));
                return;
            }

            if let Err(error) = self.on_command(command).await {
                let _ = self.event_tx.send(CoordinatorEvent::Error(error));
            }
        }
    }

    async fn on_command(
        &mut self,
        command: StorageTrieCommand,
    ) -> Result<(), ParallelStateRootError> {
        let should_drive = match command {
            StorageTrieCommand::TouchSlots(slots) => {
                for slot in slots {
                    insert_leaf_update(&mut self.updates, slot, LeafUpdate::Touched);
                }
                false
            }
            StorageTrieCommand::ApplyHashedStorage(storage) => {
                if storage.wiped {
                    return Err(ParallelStateRootError::Other(
                        "async BAL sparse trie prototype does not support storage wipes yet"
                            .to_string(),
                    ));
                }

                for (slot, value) in storage.storage {
                    let encoded = if value.is_zero() {
                        Vec::new()
                    } else {
                        alloy_rlp::encode_fixed_size(&value).to_vec()
                    };
                    insert_leaf_update(&mut self.updates, slot, LeafUpdate::Changed(encoded));
                }
                self.needs_root = true;
                self.root = None;
                false
            }
            StorageTrieCommand::Reveal(mut nodes) => {
                self.trie
                    .reveal_v2_proof_nodes(&mut nodes, true)
                    .map_err(to_parallel_sparse_error)?;
                self.deferred.proof_nodes_bufs.push(nodes);
                true
            }
            StorageTrieCommand::Drive => true,
            StorageTrieCommand::ReturnTrie { .. } => unreachable!("handled by run"),
        };

        if should_drive {
            self.drive().await
        } else {
            Ok(())
        }
    }

    #[instrument(
        name = "async_sr_storage_drive",
        level = "debug",
        target = "engine::tree::payload_processor::async_sparse_trie",
        skip_all,
        fields(
            address = ?self.address,
            updates = self.updates.len(),
            needs_root = self.needs_root,
            root_cached = self.root.is_some(),
        )
    )]
    async fn drive(&mut self) -> Result<(), ParallelStateRootError> {
        loop {
            if self.updates.is_empty() {
                if self.needs_root && self.root.is_none() {
                    let _span = debug_span!(
                        target: "engine::tree::payload_processor::async_sparse_trie",
                        "async_sr_storage_root",
                        address = ?self.address,
                    )
                    .entered();
                    let root = self.trie.root().ok_or_else(|| {
                        ParallelStateRootError::Other(format!(
                            "storage trie for {:?} remained blind after updates drained",
                            self.address
                        ))
                    })?;
                    self.root = Some(root);
                    let _ = self
                        .event_tx
                        .send(CoordinatorEvent::StorageRootReady { address: self.address, root });
                }
                return Ok(())
            }

            let updates_len_before = self.updates.len();
            let mut chunk = drain_update_chunk(&mut self.updates, MAX_STORAGE_UPDATES_PER_DRIVE);
            let mut targets = Vec::new();
            self.trie
                .update_leaves(&mut chunk, |path, min_len| {
                    targets.push(ProofV2Target::new(path).with_min_len(min_len));
                })
                .map_err(to_parallel_sparse_error)?;
            merge_leaf_updates(&mut self.updates, chunk);

            if !targets.is_empty() {
                self.proof_tx
                    .send(ProofServiceCommand::StorageTargets { address: self.address, targets })
                    .map_err(|_| {
                        ParallelStateRootError::Other(
                            "async sparse trie proof service dropped".to_string(),
                        )
                    })?;
                return Ok(())
            }

            if self.updates.len() == updates_len_before {
                return Err(ParallelStateRootError::Other(format!(
                    "storage sparse trie actor for {:?} made no progress without proof targets",
                    self.address
                )));
            }
        }
    }
}

enum ProofServiceCommand {
    AccountTargets(Vec<ProofV2Target>),
    StorageTargets { address: B256, targets: Vec<ProofV2Target> },
    RegisterStorageActor { address: B256, tx: UnboundedSender<StorageTrieCommand> },
}

struct ProofService {
    proof_worker_handle: ProofWorkerHandle,
    proof_result_tx: CrossbeamSender<ProofResultMessage>,
    command_rx: UnboundedReceiver<ProofServiceCommand>,
    proof_rx: UnboundedReceiver<ProofResultMessage>,
    account_tx: UnboundedSender<AccountTrieCommand>,
    storage_txs: B256Map<UnboundedSender<StorageTrieCommand>>,
    event_tx: UnboundedSender<CoordinatorEvent>,
    pending_targets: PendingTargets,
    fetched_account_targets: B256Map<u8>,
    fetched_storage_targets: B256Map<B256Map<u8>>,
    chunk_size: usize,
    max_targets_for_chunking: usize,
    inflight: usize,
    commands_closed: bool,
}

impl ProofService {
    #[expect(clippy::too_many_arguments)]
    fn new(
        proof_worker_handle: ProofWorkerHandle,
        proof_result_tx: CrossbeamSender<ProofResultMessage>,
        command_rx: UnboundedReceiver<ProofServiceCommand>,
        proof_rx: UnboundedReceiver<ProofResultMessage>,
        account_tx: UnboundedSender<AccountTrieCommand>,
        event_tx: UnboundedSender<CoordinatorEvent>,
        chunk_size: usize,
    ) -> Self {
        Self {
            proof_worker_handle,
            proof_result_tx,
            command_rx,
            proof_rx,
            account_tx,
            storage_txs: B256Map::default(),
            event_tx,
            pending_targets: PendingTargets::default(),
            fetched_account_targets: B256Map::default(),
            fetched_storage_targets: B256Map::default(),
            chunk_size,
            max_targets_for_chunking: DEFAULT_MAX_TARGETS_FOR_CHUNKING,
            inflight: 0,
            commands_closed: false,
        }
    }

    #[instrument(
        name = "async_sr_proof_service_run",
        level = "debug",
        target = "engine::tree::payload_processor::async_sparse_trie",
        skip_all
    )]
    async fn run(mut self) {
        loop {
            if self.commands_closed && !self.pending_targets.is_empty() {
                self.dispatch_pending_targets();
                continue;
            }

            if self.commands_closed && self.inflight == 0 && self.pending_targets.is_empty() {
                return
            }

            tokio::select! {
                maybe_command = self.command_rx.recv(), if !self.commands_closed => {
                    match maybe_command {
                        Some(command) => {
                            self.on_command(command);
                            self.drain_ready_commands();
                            if !self.pending_targets.is_empty() {
                                tokio::task::yield_now().await;
                                self.drain_ready_commands();
                                self.dispatch_pending_targets();
                            }
                        }
                        None => self.commands_closed = true,
                    }
                }
                maybe_proof = self.proof_rx.recv(), if self.inflight > 0 => {
                    match maybe_proof {
                        Some(proof) => self.on_proof_result(proof),
                        None => {
                            let _ = self.event_tx.send(CoordinatorEvent::Error(
                                ParallelStateRootError::Other(
                                    "async sparse trie proof result bridge dropped".to_string(),
                                ),
                            ));
                            return;
                        }
                    }
                }
            }
        }
    }

    fn drain_ready_commands(&mut self) {
        loop {
            match self.command_rx.try_recv() {
                Ok(command) => self.on_command(command),
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    self.commands_closed = true;
                    return
                }
            }
        }
    }

    fn on_command(&mut self, command: ProofServiceCommand) {
        match command {
            ProofServiceCommand::AccountTargets(targets) => {
                self.queue_account_targets(targets);
            }
            ProofServiceCommand::StorageTargets { address, targets } => {
                self.queue_storage_targets(address, targets);
            }
            ProofServiceCommand::RegisterStorageActor { address, tx } => {
                self.storage_txs.insert(address, tx);
            }
        }
    }

    fn queue_account_targets(&mut self, targets: Vec<ProofV2Target>) {
        let mut queued = 0usize;
        let mut retry_if_idle = Vec::new();

        for target in targets {
            match self.fetched_account_targets.entry(target.key()) {
                Entry::Occupied(mut entry) => {
                    if target.min_len < *entry.get() {
                        entry.insert(target.min_len);
                        self.pending_targets.push_account_target(target);
                        queued += 1;
                    } else {
                        retry_if_idle.push(target);
                    }
                }
                Entry::Vacant(entry) => {
                    entry.insert(target.min_len);
                    self.pending_targets.push_account_target(target);
                    queued += 1;
                }
            }
        }

        if queued == 0 && self.inflight == 0 && self.pending_targets.is_empty() {
            for target in retry_if_idle {
                self.fetched_account_targets.insert(target.key(), target.min_len);
                self.pending_targets.push_account_target(target);
            }
        }
    }

    fn queue_storage_targets(&mut self, address: B256, targets: Vec<ProofV2Target>) {
        let fetched = self.fetched_storage_targets.entry(address).or_default();
        let mut queued = Vec::new();
        let mut retry_if_idle = Vec::new();
        for target in targets {
            match fetched.entry(target.key()) {
                Entry::Occupied(mut entry) => {
                    if target.min_len < *entry.get() {
                        entry.insert(target.min_len);
                        queued.push(target);
                    } else {
                        retry_if_idle.push(target);
                    }
                }
                Entry::Vacant(entry) => {
                    entry.insert(target.min_len);
                    queued.push(target);
                }
            }
        }

        if !queued.is_empty() {
            self.pending_targets.extend_storage_targets(&address, queued);
        } else if self.inflight == 0 && self.pending_targets.is_empty() && !retry_if_idle.is_empty()
        {
            let fetched = self.fetched_storage_targets.entry(address).or_default();
            for target in &retry_if_idle {
                fetched.insert(target.key(), target.min_len);
            }
            self.pending_targets.extend_storage_targets(&address, retry_if_idle);
        }
    }

    fn on_proof_result(&mut self, proof: ProofResultMessage) {
        self.inflight = self.inflight.saturating_sub(1);

        let decoded = match proof.result {
            Ok(decoded) => decoded,
            Err(error) => {
                let _ = self.event_tx.send(CoordinatorEvent::Error(error));
                return
            }
        };

        let DecodedMultiProofV2 { account_proofs, storage_proofs } = decoded;
        let storage_tries = storage_proofs.len();
        let storage_proof_nodes = storage_proofs.values().map(|nodes| nodes.len()).sum::<usize>();
        let _span = debug_span!(
            target: "engine::tree::payload_processor::async_sparse_trie",
            "async_sr_proof_result",
            account_proof_nodes = account_proofs.len(),
            storage_tries,
            storage_proof_nodes,
            inflight = self.inflight,
        )
        .entered();

        if !account_proofs.is_empty() &&
            self.account_tx.send(AccountTrieCommand::Reveal(account_proofs)).is_err()
        {
            let _ = self.event_tx.send(CoordinatorEvent::Error(ParallelStateRootError::Other(
                "async account sparse trie actor dropped before proof reveal".to_string(),
            )));
        }

        for (address, proof_nodes) in storage_proofs {
            let Some(tx) = self.storage_txs.get(&address) else {
                let _ = self.event_tx.send(CoordinatorEvent::Error(ParallelStateRootError::Other(
                    format!("received storage proof for unregistered actor {address:?}"),
                )));
                continue;
            };

            if tx.send(StorageTrieCommand::Reveal(proof_nodes)).is_err() {
                let _ = self.event_tx.send(CoordinatorEvent::Error(ParallelStateRootError::Other(
                    format!(
                    "async storage sparse trie actor for {address:?} dropped before proof reveal"
                ),
                )));
            }
        }
    }

    fn dispatch_pending_targets(&mut self) {
        if self.pending_targets.is_empty() {
            return;
        }

        let (targets, chunking_length) = self.pending_targets.take();
        let account_targets = targets.account_targets.len();
        let storage_tries = targets.storage_targets.len();
        let storage_targets =
            targets.storage_targets.values().map(|targets| targets.len()).sum::<usize>();
        let _span = debug_span!(
            target: "engine::tree::payload_processor::async_sparse_trie",
            "async_sr_dispatch_pending_targets",
            chunking_length,
            account_targets,
            storage_tries,
            storage_targets,
            inflight = self.inflight,
        )
        .entered();
        let proof_worker_handle = self.proof_worker_handle.clone();
        let proof_result_tx = self.proof_result_tx.clone();
        let event_tx = self.event_tx.clone();
        let has_multiple_idle_account_workers =
            self.proof_worker_handle.has_multiple_idle_account_workers();
        let has_multiple_idle_storage_workers =
            self.proof_worker_handle.has_multiple_idle_storage_workers();
        let mut dispatched = 0usize;

        dispatch_with_chunking(
            targets,
            chunking_length,
            self.chunk_size,
            self.max_targets_for_chunking,
            has_multiple_idle_account_workers,
            has_multiple_idle_storage_workers,
            MultiProofTargetsV2::chunks,
            |proof_targets| {
                dispatched += 1;
                if let Err(error) =
                    proof_worker_handle.dispatch_account_multiproof(AccountMultiproofInput {
                        targets: proof_targets,
                        proof_result_sender: ProofResultContext::new(
                            proof_result_tx.clone(),
                            HashedPostState::default(),
                            Instant::now(),
                        ),
                    })
                {
                    error!("failed to dispatch async sparse trie account multiproof: {error:?}");
                    let _ = event_tx.send(CoordinatorEvent::Error(error.into()));
                }
            },
        );

        self.inflight += dispatched;
    }
}

#[derive(Default)]
struct PendingTargets {
    targets: MultiProofTargetsV2,
    len: usize,
}

impl PendingTargets {
    const fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn take(&mut self) -> (MultiProofTargetsV2, usize) {
        (std::mem::take(&mut self.targets), std::mem::take(&mut self.len))
    }

    fn push_account_target(&mut self, target: ProofV2Target) {
        self.targets.account_targets.push(target);
        self.len += 1;
    }

    fn extend_storage_targets(&mut self, address: &B256, targets: Vec<ProofV2Target>) {
        self.len += targets.len();
        self.targets.storage_targets.entry(*address).or_default().extend(targets);
    }
}

struct BridgeHandle {
    stop: Arc<AtomicBool>,
    handle: Option<StdJoinHandle<()>>,
}

impl BridgeHandle {
    fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn spawn_state_message_bridge(
    rx: CrossbeamReceiver<StateRootMessage>,
    tx: UnboundedSender<StateRootMessage>,
) -> BridgeHandle {
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    let handle = thread::Builder::new()
        .name("async-sr-state-bridge".to_string())
        .spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                match rx.recv_timeout(BRIDGE_RECV_TIMEOUT) {
                    Ok(message) => {
                        let finished = matches!(message, StateRootMessage::FinishedStateUpdates);
                        if tx.send(message).is_err() || finished {
                            return;
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
        })
        .ok();

    BridgeHandle { stop, handle }
}

fn spawn_proof_result_bridge(
    rx: CrossbeamReceiver<ProofResultMessage>,
    tx: UnboundedSender<ProofResultMessage>,
) -> BridgeHandle {
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    let handle = thread::Builder::new()
        .name("async-sr-proof-bridge".to_string())
        .spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                match rx.recv_timeout(BRIDGE_RECV_TIMEOUT) {
                    Ok(message) => {
                        if tx.send(message).is_err() {
                            return;
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
        })
        .ok();

    BridgeHandle { stop, handle }
}

fn send_storage_command(
    tx: &UnboundedSender<StorageTrieCommand>,
    command: StorageTrieCommand,
) -> Result<(), ParallelStateRootError> {
    tx.send(command).map_err(|_| {
        ParallelStateRootError::Other("async storage sparse trie actor dropped".to_string())
    })
}

fn insert_leaf_update(updates: &mut B256Map<LeafUpdate>, key: B256, update: LeafUpdate) {
    match updates.entry(key) {
        Entry::Occupied(mut entry) => {
            if update.is_changed() || !entry.get().is_changed() {
                entry.insert(update);
            }
        }
        Entry::Vacant(entry) => {
            entry.insert(update);
        }
    }
}

fn merge_leaf_updates(updates: &mut B256Map<LeafUpdate>, chunk: B256Map<LeafUpdate>) {
    for (key, update) in chunk {
        insert_leaf_update(updates, key, update);
    }
}

fn drain_update_chunk(updates: &mut B256Map<LeafUpdate>, limit: usize) -> B256Map<LeafUpdate> {
    let len = updates.len().min(limit);
    let keys: Vec<_> = updates.keys().take(len).copied().collect();
    let mut chunk = B256Map::default();
    chunk.reserve(len);

    for key in keys {
        if let Some(update) = updates.remove(&key) {
            chunk.insert(key, update);
        }
    }

    chunk
}

fn encode_account_leaf_value(
    account: Option<Account>,
    storage_root: B256,
    account_rlp_buf: &mut Vec<u8>,
) -> Vec<u8> {
    if account.is_none_or(|account| account.is_empty()) && storage_root == EMPTY_ROOT_HASH {
        return Vec::new();
    }

    account_rlp_buf.clear();
    account.unwrap_or_default().into_trie_account(storage_root).encode(account_rlp_buf);
    account_rlp_buf.clone()
}

fn to_parallel_sparse_error(error: impl std::fmt::Debug) -> ParallelStateRootError {
    ParallelStateRootError::Other(format!("async sparse trie operation failed: {error:?}"))
}
