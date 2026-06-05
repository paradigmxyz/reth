//! Multiproof task related functionality.

use metrics::{Gauge, Histogram};
use reth_metrics::Metrics;

pub use reth_trie_parallel::state_root_task::{
    evm_state_to_hashed_post_state, StateHookSender, StateRootComputeOutcome, StateRootHandle,
    StateRootMessage,
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
#[expect(clippy::too_many_arguments)]
pub(crate) fn dispatch_with_chunking<T, I>(
    items: T,
    chunking_len: usize,
    chunk_size: usize,
    max_targets_for_chunking: usize,
    has_multiple_idle_workers: bool,
    chunker: impl FnOnce(T, usize) -> I,
    mut dispatch: impl FnMut(T),
) where
    I: IntoIterator<Item = T>,
{
    let has_full_chunks = chunking_len >= chunk_size.saturating_mul(2);
    let should_chunk =
        chunking_len > max_targets_for_chunking || (has_full_chunks && has_multiple_idle_workers);

    if should_chunk && chunking_len > chunk_size {
        for chunk in chunker(items, chunk_size) {
            dispatch(chunk);
        }
        return;
    }

    dispatch(items);
}

/// Returns true when worker availability can affect a chunking decision.
pub(crate) const fn chunking_can_use_idle_workers(
    chunking_len: usize,
    chunk_size: usize,
    max_targets_for_chunking: usize,
) -> bool {
    chunking_len <= max_targets_for_chunking && chunking_len >= chunk_size.saturating_mul(2)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk_vec(items: Vec<u8>, size: usize) -> Vec<Vec<u8>> {
        items.chunks(size).map(<[u8]>::to_vec).collect()
    }

    #[test]
    fn forced_chunking_does_not_need_idle_workers() {
        let mut dispatched = Vec::new();
        dispatch_with_chunking(vec![1, 2, 3, 4], 4, 2, 3, false, chunk_vec, |chunk| {
            dispatched.push(chunk);
        });

        assert_eq!(dispatched, vec![vec![1, 2], vec![3, 4]]);
    }

    #[test]
    fn idle_workers_only_matter_for_full_non_forced_chunks() {
        assert!(!chunking_can_use_idle_workers(4, 2, 3));
        assert!(!chunking_can_use_idle_workers(3, 2, 8));
        assert!(chunking_can_use_idle_workers(4, 2, 8));
    }
}
