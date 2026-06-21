//! Multiproof task related functionality.

use metrics::{Gauge, Histogram};
use reth_metrics::Metrics;

pub use reth_trie_parallel::state_root_task::{
    evm_state_to_hashed_post_state, StateHookSender, StateRootComputeOutcome, StateRootHandle,
    StateRootMessage,
};

/// The default max targets, for limiting the number of account and storage proof targets to be
/// fetched by a single worker. If exceeded, chunking is forced regardless of worker availability.
pub(crate) const DEFAULT_MAX_TARGETS_FOR_CHUNKING: usize = 64;

/// Minimum chunk size used when a target batch is forced to split.
const MIN_FORCED_CHUNK_SIZE: usize = 32;

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
    has_multiple_idle_account_workers: bool,
    has_multiple_idle_storage_workers: bool,
    chunker: impl FnOnce(T, usize) -> I,
    mut dispatch: impl FnMut(T),
) where
    I: IntoIterator<Item = T>,
{
    let has_full_chunks = chunking_len >= chunk_size.saturating_mul(2);
    let force_chunk = chunking_len > max_targets_for_chunking;
    let should_chunk_for_idle =
        has_full_chunks && (has_multiple_idle_account_workers || has_multiple_idle_storage_workers);

    if force_chunk && chunking_len > chunk_size {
        let forced_chunk_size = chunk_size.max(MIN_FORCED_CHUNK_SIZE);
        for chunk in chunker(items, forced_chunk_size) {
            dispatch(chunk);
        }
        return;
    }

    if should_chunk_for_idle && chunking_len > chunk_size {
        for chunk in chunker(items, chunk_size) {
            dispatch(chunk);
        }
        return;
    }

    dispatch(items);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vec_chunks(items: Vec<usize>, chunk_size: usize) -> Vec<Vec<usize>> {
        items.chunks(chunk_size).map(<[usize]>::to_vec).collect()
    }

    #[test]
    fn dispatches_single_batch_below_forced_limit_without_idle_workers() {
        let items: Vec<_> = (0..60).collect();
        let mut dispatched = Vec::new();

        dispatch_with_chunking(
            items,
            60,
            5,
            DEFAULT_MAX_TARGETS_FOR_CHUNKING,
            false,
            false,
            vec_chunks,
            |chunk| dispatched.push(chunk),
        );

        assert_eq!(dispatched.len(), 1);
        assert_eq!(dispatched[0].len(), 60);
    }

    #[test]
    fn idle_workers_use_configured_small_chunks() {
        let items: Vec<_> = (0..12).collect();
        let mut dispatched = Vec::new();

        dispatch_with_chunking(
            items,
            12,
            5,
            DEFAULT_MAX_TARGETS_FOR_CHUNKING,
            true,
            false,
            vec_chunks,
            |chunk| dispatched.push(chunk),
        );

        let lengths: Vec<_> = dispatched.iter().map(Vec::len).collect();
        assert_eq!(lengths, vec![5, 5, 2]);
    }

    #[test]
    fn forced_large_batches_use_medium_chunks() {
        let items: Vec<_> = (0..96).collect();
        let mut dispatched = Vec::new();

        dispatch_with_chunking(
            items,
            96,
            5,
            DEFAULT_MAX_TARGETS_FOR_CHUNKING,
            false,
            false,
            vec_chunks,
            |chunk| dispatched.push(chunk),
        );

        let lengths: Vec<_> = dispatched.iter().map(Vec::len).collect();
        assert_eq!(lengths, vec![32, 32, 32]);
    }

    #[test]
    fn forced_large_batches_honor_larger_configured_chunks() {
        let items: Vec<_> = (0..96).collect();
        let mut dispatched = Vec::new();

        dispatch_with_chunking(
            items,
            96,
            40,
            DEFAULT_MAX_TARGETS_FOR_CHUNKING,
            false,
            false,
            vec_chunks,
            |chunk| dispatched.push(chunk),
        );

        let lengths: Vec<_> = dispatched.iter().map(Vec::len).collect();
        assert_eq!(lengths, vec![40, 40, 16]);
    }
}
