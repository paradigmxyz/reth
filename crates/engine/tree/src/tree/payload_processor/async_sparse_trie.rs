//! Experimental Tokio actor based sparse trie task for BAL state updates.

use crate::tree::payload_processor::{
    multiproof::{MultiProofTaskMetrics, StateRootComputeOutcome, StateRootMessage},
    preserved_sparse_trie::{PreservedSparseTrie, SharedPreservedSparseTrie},
    SPARSE_TRIE_MAX_NODES_SHRINK_CAPACITY, SPARSE_TRIE_MAX_VALUES_SHRINK_CAPACITY,
};
use alloy_primitives::{
    map::{B256Map, Entry},
    B256,
};
use alloy_rlp::Encodable;
use crossbeam_channel::Receiver as CrossbeamReceiver;
use reth_primitives_traits::{Account, FastInstant as Instant};
use reth_tasks::Runtime;
use reth_trie::{HashedPostState, EMPTY_ROOT_HASH};
use reth_trie_parallel::{proof_task::ProofWorkerHandle, root::ParallelStateRootError};
use reth_trie_sparse::{
    ArenaParallelSparseTrie, ConfigurableSparseTrie, LeafUpdate, RevealableSparseTrie,
    SparseStateTrie,
};
use std::{
    future::Future,
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc, Arc,
    },
    time::Duration,
};
use tokio::{
    runtime::{Builder, Runtime as TokioRuntime},
    sync::mpsc::UnboundedSender,
};
use tracing::{debug, debug_span, instrument};

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

mod account_actor;
mod bridge;
mod coordinator;
mod proof_service;
mod storage_actor;

use coordinator::{AsyncSparseTrieCoordinator, CoordinatorOutput};
use storage_actor::StorageTrieCommand;

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
        final_hashed_state_tx,
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
    final_hashed_state_tx: mpsc::Sender<HashedPostState>,
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

    if task_result.is_some() {
        let final_hashed_state = std::mem::take(&mut output.final_hashed_state);
        let _ = final_hashed_state_tx.send(final_hashed_state);
    }

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
