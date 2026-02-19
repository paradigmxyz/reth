//! Multiproof task related functionality.

use alloy_evm::block::StateChangeSource;
use alloy_primitives::{keccak256, B256};
use crossbeam_channel::Sender as CrossbeamSender;
use derive_more::derive::Deref;
use metrics::{Gauge, Histogram};
use reth_metrics::Metrics;
use reth_revm::state::EvmState;
use reth_trie::{HashedPostState, HashedStorage};
use reth_trie_parallel::targets_v2::MultiProofTargetsV2;
use std::sync::Arc;
use tracing::trace;

/// Source of state changes, either from EVM execution or from a Block Access List.
#[derive(Clone, Copy)]
pub enum Source {
    /// State changes from EVM execution.
    Evm(StateChangeSource),
    /// State changes from Block Access List (EIP-7928).
    BlockAccessList,
}

impl std::fmt::Debug for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Evm(source) => source.fmt(f),
            Self::BlockAccessList => f.write_str("BlockAccessList"),
        }
    }
}

impl From<StateChangeSource> for Source {
    fn from(source: StateChangeSource) -> Self {
        Self::Evm(source)
    }
}

/// The default max targets, for limiting the number of account and storage proof targets to be
/// fetched by a single worker. If exceeded, chunking is forced regardless of worker availability.
pub(crate) const DEFAULT_MAX_TARGETS_FOR_CHUNKING: usize = 300;

/// Messages used internally by the multi proof task.
#[derive(Debug)]
pub enum MultiProofMessage {
    /// Prefetch proof targets
    PrefetchProofs(MultiProofTargetsV2),
    /// New state update from transaction execution with its source
    StateUpdate(Source, EvmState),
    /// State update that can be applied to the sparse trie without any new proofs.
    ///
    /// It can be the case when all accounts and storage slots from the state update were already
    /// fetched and revealed.
    EmptyProof {
        /// The index of this proof in the sequence of state updates
        sequence_number: u64,
        /// The state update that was used to calculate the proof
        state: HashedPostState,
    },
    /// Pre-hashed state update from BAL conversion that can be applied directly without proofs.
    HashedStateUpdate(HashedPostState),
    /// Block Access List (EIP-7928; BAL) containing complete state changes for the block.
    ///
    /// When received, the task generates a single state update from the BAL and processes it.
    /// No further messages are expected after receiving this variant.
    BlockAccessList(Arc<alloy_eip7928::BlockAccessList>),
    /// Signals state update stream end.
    ///
    /// This is triggered by block execution, indicating that no additional state updates are
    /// expected.
    FinishedStateUpdates,
}

/// A wrapper for the sender that signals completion when dropped.
///
/// This type is intended to be used in combination with the evm executor statehook.
/// This should trigger once the block has been executed (after) the last state update has been
/// sent. This triggers the exit condition of the multi proof task.
#[derive(Deref, Debug)]
pub(super) struct StateHookSender(CrossbeamSender<MultiProofMessage>);

impl StateHookSender {
    pub(crate) const fn new(inner: CrossbeamSender<MultiProofMessage>) -> Self {
        Self(inner)
    }
}

impl Drop for StateHookSender {
    fn drop(&mut self) {
        // Send completion signal when the sender is dropped
        let _ = self.0.send(MultiProofMessage::FinishedStateUpdates);
    }
}

pub(crate) fn evm_state_to_hashed_post_state(update: EvmState) -> HashedPostState {
    let mut hashed_state = HashedPostState::with_capacity(update.len());

    for (address, account) in update {
        if account.is_touched() {
            let hashed_address = keccak256(address);
            trace!(target: "engine::tree::payload_processor::multiproof", ?address, ?hashed_address, "Adding account to state update");

            let destroyed = account.is_selfdestructed();
            let info = if destroyed { None } else { Some(account.info.into()) };
            hashed_state.accounts.insert(hashed_address, info);

            let mut changed_storage_iter = account
                .storage
                .into_iter()
                .filter(|(_slot, value)| value.is_changed())
                .map(|(slot, value)| (keccak256(B256::from(slot)), value.present_value))
                .peekable();

            if destroyed {
                hashed_state.storages.insert(hashed_address, HashedStorage::new(true));
            } else if changed_storage_iter.peek().is_some() {
                hashed_state
                    .storages
                    .insert(hashed_address, HashedStorage::from_iter(false, changed_storage_iter));
            }
        }
    }

    hashed_state
}

#[derive(Metrics, Clone)]
#[metrics(scope = "tree.root")]
pub(crate) struct MultiProofTaskMetrics {
    /// Histogram of active storage workers processing proofs.
    pub active_storage_workers_histogram: Histogram,
    /// Histogram of active account workers processing proofs.
    pub active_account_workers_histogram: Histogram,
    /// Gauge for the maximum number of storage workers in the pool.
    pub max_storage_workers: Gauge,
    /// Gauge for the maximum number of account workers in the pool.
    pub max_account_workers: Gauge,
    /// Histogram of pending storage multiproofs in the queue.
    pub pending_storage_multiproofs_histogram: Histogram,
    /// Histogram of pending account multiproofs in the queue.
    pub pending_account_multiproofs_histogram: Histogram,

    /// Histogram of the number of prefetch proof target accounts.
    pub prefetch_proof_targets_accounts_histogram: Histogram,
    /// Histogram of the number of prefetch proof target storages.
    pub prefetch_proof_targets_storages_histogram: Histogram,
    /// Histogram of the number of prefetch proof target chunks.
    pub prefetch_proof_chunks_histogram: Histogram,

    /// Histogram of the number of state update proof target accounts.
    pub state_update_proof_targets_accounts_histogram: Histogram,
    /// Histogram of the number of state update proof target storages.
    pub state_update_proof_targets_storages_histogram: Histogram,
    /// Histogram of the number of state update proof target chunks.
    pub state_update_proof_chunks_histogram: Histogram,

    /// Histogram of prefetch proof batch sizes (number of messages merged).
    pub prefetch_batch_size_histogram: Histogram,

    /// Histogram of proof calculation durations.
    pub proof_calculation_duration_histogram: Histogram,

    /// Histogram of sparse trie update durations.
    pub sparse_trie_update_duration_histogram: Histogram,
    /// Time spent revealing multiproof results into the sparse trie.
    pub sparse_trie_reveal_multiproof_duration_histogram: Histogram,
    /// Time spent coalescing multiple proof results from the channel.
    pub sparse_trie_proof_coalesce_duration_histogram: Histogram,
    /// Time the event loop spent blocked waiting on channels (idle time).
    pub sparse_trie_channel_wait_duration_histogram: Histogram,
    /// Time spent in `process_new_updates` + `promote_pending_account_updates` (trie mutations).
    pub sparse_trie_process_updates_duration_histogram: Histogram,
    /// Histogram of sparse trie final update durations.
    pub sparse_trie_final_update_duration_histogram: Histogram,
    /// Histogram of sparse trie total durations.
    pub sparse_trie_total_duration_histogram: Histogram,

    /// Histogram of state updates received.
    pub state_updates_received_histogram: Histogram,
    /// Histogram of proofs processed.
    pub proofs_processed_histogram: Histogram,
    /// Histogram of total time spent in the multiproof task.
    pub multiproof_task_total_duration_histogram: Histogram,
    /// Total time spent waiting for the first state update or prefetch request.
    pub first_update_wait_time_histogram: Histogram,
    /// Total time spent waiting for the last proof result.
    pub last_proof_wait_time_histogram: Histogram,
    /// Time spent preparing the sparse trie for reuse after state root computation.
    pub into_trie_for_reuse_duration_histogram: Histogram,
    /// Time spent waiting for preserved sparse trie cache to become available.
    pub sparse_trie_cache_wait_duration_histogram: Histogram,
}

/// Dispatches work items as a single unit or in chunks based on target size and worker
/// availability.
#[allow(clippy::too_many_arguments)]
pub(crate) fn dispatch_with_chunking<T, I>(
    items: T,
    chunking_len: usize,
    chunk_size: Option<usize>,
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

    if should_chunk &&
        let Some(chunk_size) = chunk_size &&
        chunking_len > chunk_size
    {
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
