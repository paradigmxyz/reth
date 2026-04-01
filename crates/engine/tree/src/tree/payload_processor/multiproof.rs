//! Multiproof task related functionality.

use metrics::{Gauge, Histogram};
use reth_metrics::Metrics;

pub use reth_trie_parallel::state_root_task::{
    evm_state_to_hashed_post_state, Source, StateHookSender, StateRootComputeOutcome,
    StateRootHandle, StateRootMessage,
};

/// The default max targets, for limiting the number of account and storage proof targets to be
/// fetched by a single worker. If exceeded, chunking is forced regardless of worker availability.
pub(crate) const DEFAULT_MAX_TARGETS_FOR_CHUNKING: usize = 300;

#[derive(Metrics, Clone)]
#[metrics(scope = "tree.root")]
pub(crate) struct MultiProofTaskMetrics {
    /// Histogram of durations spent revealing multiproof results into the sparse trie.
    pub sparse_trie_reveal_multiproof_duration_histogram: Histogram,
    /// Histogram of durations spent coalescing multiple proof results from the channel.
    pub sparse_trie_proof_coalesce_duration_histogram: Histogram,
    /// Histogram of durations the event loop spent blocked waiting on channels.
    pub sparse_trie_channel_wait_duration_histogram: Histogram,
    /// Histogram of durations spent processing trie updates and promoting pending accounts.
    pub sparse_trie_process_updates_duration_histogram: Histogram,
    /// Histogram of sparse trie final update durations.
    pub sparse_trie_final_update_duration_histogram: Histogram,
    /// Histogram of sparse trie total durations.
    pub sparse_trie_total_duration_histogram: Histogram,
    /// Time spent preparing the sparse trie for reuse after state root computation.
    pub into_trie_for_reuse_duration_histogram: Histogram,
    /// Time spent waiting for preserved sparse trie cache to become available.
    pub sparse_trie_cache_wait_duration_histogram: Histogram,
    /// Histogram for sparse trie task idle time in seconds (waiting for updates or proof
    /// results). Excludes the final wait after the channel is closed.
    pub sparse_trie_idle_time_seconds: Histogram,
    /// Histogram for hashing task idle time in seconds (waiting for messages from execution).
    /// Excludes the final wait after the channel is closed.
    pub hashing_task_idle_time_seconds: Histogram,

    /// Number of account leaf updates applied without needing a new proof (cache hits).
    pub sparse_trie_account_cache_hits: Histogram,
    /// Number of account leaf updates that required a new proof (cache misses).
    pub sparse_trie_account_cache_misses: Histogram,
    /// Number of storage leaf updates applied without needing a new proof (cache hits).
    pub sparse_trie_storage_cache_hits: Histogram,
    /// Number of storage leaf updates that required a new proof (cache misses).
    pub sparse_trie_storage_cache_misses: Histogram,

    /// Retained memory of the preserved sparse trie cache in bytes.
    pub sparse_trie_retained_memory_bytes: Gauge,
    /// Number of storage tries retained in the preserved sparse trie cache.
    pub sparse_trie_retained_storage_tries: Gauge,
}

/// Dispatches work items as a single unit or in chunks based on target size and worker
/// availability.
#[allow(clippy::too_many_arguments)]
pub(crate) fn dispatch_with_chunking<T, I>(
    items: T,
    chunking_len: usize,
    chunk_size: usize,
    max_targets_for_chunking: usize,
    available_account_workers: usize,
    available_storage_workers: usize,
    chunker: impl FnOnce(T, usize) -> I,
    mut dispatch: impl FnMut(T),
) -> usize
where
    I: IntoIterator<Item = T>,
{
    let should_chunk = chunking_len > max_targets_for_chunking ||
        available_account_workers > 1 ||
        available_storage_workers > 1;

    if should_chunk && chunking_len > chunk_size {
        let mut num_chunks = 0usize;
        for chunk in chunker(items, chunk_size) {
            dispatch(chunk);
            num_chunks += 1;
        }
        return num_chunks;
    }

    dispatch(items);
    1
}
