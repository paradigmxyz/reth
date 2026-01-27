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

#[cfg(test)]
mod tests {
    use crate::tree::cached_state::CachedStateProvider;

    use super::*;
    use alloy_eip7928::{AccountChanges, BalanceChange};
    use alloy_primitives::Address;
    use reth_provider::{
        providers::OverlayStateProviderFactory, test_utils::create_test_provider_factory,
        BlockNumReader, BlockReader, ChangeSetReader, DatabaseProviderFactory, LatestStateProvider,
        PruneCheckpointReader, StageCheckpointReader, StateProviderBox, StorageChangeSetReader,
    };
    use reth_trie::MultiProof;
    use reth_trie_db::ChangesetCache;
    use reth_trie_parallel::proof_task::{ProofTaskCtx, ProofWorkerHandle};
    use revm_primitives::{B256, U256};
    use std::sync::{Arc, OnceLock};
    use tokio::runtime::{Handle, Runtime};

    /// Get a handle to the test runtime, creating it if necessary
    fn get_test_runtime_handle() -> Handle {
        static TEST_RT: OnceLock<Runtime> = OnceLock::new();
        TEST_RT
            .get_or_init(|| {
                tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap()
            })
            .handle()
            .clone()
    }

    fn create_test_state_root_task<F>(factory: F) -> MultiProofTask
    where
        F: DatabaseProviderFactory<
                Provider: BlockReader
                              + StageCheckpointReader
                              + PruneCheckpointReader
                              + ChangeSetReader
                              + StorageChangeSetReader
                              + BlockNumReader,
            > + Clone
            + Send
            + 'static,
    {
        let rt_handle = get_test_runtime_handle();
        let changeset_cache = ChangesetCache::new();
        let overlay_factory = OverlayStateProviderFactory::new(factory, changeset_cache);
        let task_ctx = ProofTaskCtx::new(overlay_factory);
        let proof_handle = ProofWorkerHandle::new(rt_handle, task_ctx, 1, 1, false, None);
        let (to_sparse_trie, _receiver) = std::sync::mpsc::channel();
        let (tx, rx) = crossbeam_channel::unbounded();

        MultiProofTask::new(proof_handle, to_sparse_trie, Some(1), tx, rx)
    }

    fn create_cached_provider<F>(factory: F) -> CachedStateProvider<StateProviderBox>
    where
        F: DatabaseProviderFactory<
                Provider: BlockReader + StageCheckpointReader + PruneCheckpointReader,
            > + Clone
            + Send
            + 'static,
    {
        let db_provider = factory.database_provider_ro().unwrap();
        let state_provider: StateProviderBox = Box::new(LatestStateProvider::new(db_provider));
        let cache = crate::tree::cached_state::ExecutionCache::new(1000);
        CachedStateProvider::new(state_provider, cache, Default::default())
    }

    #[test]
    fn test_add_proof_in_sequence() {
        let mut sequencer = ProofSequencer::default();
        let proof1 = MultiProof::default();
        let proof2 = MultiProof::default();
        sequencer.next_sequence = 2;

        let ready = sequencer.add_proof(0, SparseTrieUpdate::from_multiproof(proof1).unwrap());
        assert_eq!(ready.len(), 1);
        assert!(!sequencer.has_pending());

        let ready = sequencer.add_proof(1, SparseTrieUpdate::from_multiproof(proof2).unwrap());
        assert_eq!(ready.len(), 1);
        assert!(!sequencer.has_pending());
    }

    #[test]
    fn test_add_proof_out_of_order() {
        let mut sequencer = ProofSequencer::default();
        let proof1 = MultiProof::default();
        let proof2 = MultiProof::default();
        let proof3 = MultiProof::default();
        sequencer.next_sequence = 3;

        let ready = sequencer.add_proof(2, SparseTrieUpdate::from_multiproof(proof3).unwrap());
        assert_eq!(ready.len(), 0);
        assert!(sequencer.has_pending());

        let ready = sequencer.add_proof(0, SparseTrieUpdate::from_multiproof(proof1).unwrap());
        assert_eq!(ready.len(), 1);
        assert!(sequencer.has_pending());

        let ready = sequencer.add_proof(1, SparseTrieUpdate::from_multiproof(proof2).unwrap());
        assert_eq!(ready.len(), 2);
        assert!(!sequencer.has_pending());
    }

    #[test]
    fn test_add_proof_with_gaps() {
        let mut sequencer = ProofSequencer::default();
        let proof1 = MultiProof::default();
        let proof3 = MultiProof::default();
        sequencer.next_sequence = 3;

        let ready = sequencer.add_proof(0, SparseTrieUpdate::from_multiproof(proof1).unwrap());
        assert_eq!(ready.len(), 1);

        let ready = sequencer.add_proof(2, SparseTrieUpdate::from_multiproof(proof3).unwrap());
        assert_eq!(ready.len(), 0);
        assert!(sequencer.has_pending());
    }

    #[test]
    fn test_add_proof_duplicate_sequence() {
        let mut sequencer = ProofSequencer::default();
        let proof1 = MultiProof::default();
        let proof2 = MultiProof::default();

        let ready = sequencer.add_proof(0, SparseTrieUpdate::from_multiproof(proof1).unwrap());
        assert_eq!(ready.len(), 1);

        let ready = sequencer.add_proof(0, SparseTrieUpdate::from_multiproof(proof2).unwrap());
        assert_eq!(ready.len(), 0);
        assert!(!sequencer.has_pending());
    }

    #[test]
    fn test_add_proof_batch_processing() {
        let mut sequencer = ProofSequencer::default();
        let proofs: Vec<_> = (0..5).map(|_| MultiProof::default()).collect();
        sequencer.next_sequence = 5;

        sequencer.add_proof(4, SparseTrieUpdate::from_multiproof(proofs[4].clone()).unwrap());
        sequencer.add_proof(2, SparseTrieUpdate::from_multiproof(proofs[2].clone()).unwrap());
        sequencer.add_proof(1, SparseTrieUpdate::from_multiproof(proofs[1].clone()).unwrap());
        sequencer.add_proof(3, SparseTrieUpdate::from_multiproof(proofs[3].clone()).unwrap());

        let ready =
            sequencer.add_proof(0, SparseTrieUpdate::from_multiproof(proofs[0].clone()).unwrap());
        assert_eq!(ready.len(), 5);
        assert!(!sequencer.has_pending());
    }

    fn create_get_proof_targets_state() -> HashedPostState {
        let mut state = HashedPostState::default();

        let addr1 = B256::random();
        let addr2 = B256::random();
        state.accounts.insert(addr1, Some(Default::default()));
        state.accounts.insert(addr2, Some(Default::default()));

        let mut storage = HashedStorage::default();
        let slot1 = B256::random();
        let slot2 = B256::random();
        storage.storage.insert(slot1, U256::ZERO);
        storage.storage.insert(slot2, U256::from(1));
        state.storages.insert(addr1, storage);

        state
    }

    fn unwrap_legacy_targets(targets: VersionedMultiProofTargets) -> MultiProofTargets {
        match targets {
            VersionedMultiProofTargets::Legacy(targets) => targets,
            VersionedMultiProofTargets::V2(_) => panic!("Expected Legacy targets"),
        }
    }

    #[test]
    fn test_get_proof_targets_new_account_targets() {
        let state = create_get_proof_targets_state();
        let fetched = MultiProofTargets::default();

        let targets = unwrap_legacy_targets(get_proof_targets(
            &state,
            &fetched,
            &MultiAddedRemovedKeys::new(),
            false,
        ));

        // should return all accounts as targets since nothing was fetched before
        assert_eq!(targets.len(), state.accounts.len());
        for addr in state.accounts.keys() {
            assert!(targets.contains_key(addr));
        }
    }

    #[test]
    fn test_get_proof_targets_new_storage_targets() {
        let state = create_get_proof_targets_state();
        let fetched = MultiProofTargets::default();

        let targets = unwrap_legacy_targets(get_proof_targets(
            &state,
            &fetched,
            &MultiAddedRemovedKeys::new(),
            false,
        ));

        // verify storage slots are included for accounts with storage
        for (addr, storage) in &state.storages {
            assert!(targets.contains_key(addr));
            let target_slots = &targets[addr];
            assert_eq!(target_slots.len(), storage.storage.len());
            for slot in storage.storage.keys() {
                assert!(target_slots.contains(slot));
            }
        }
    }

    #[test]
    fn test_get_proof_targets_filter_already_fetched_accounts() {
        let state = create_get_proof_targets_state();
        let mut fetched = MultiProofTargets::default();

        // select an account that has no storage updates
        let fetched_addr = state
            .accounts
            .keys()
            .find(|&&addr| !state.storages.contains_key(&addr))
            .expect("Should have an account without storage");

        // mark the account as already fetched
        fetched.insert(*fetched_addr, HashSet::default());

        let targets = unwrap_legacy_targets(get_proof_targets(
            &state,
            &fetched,
            &MultiAddedRemovedKeys::new(),
            false,
        ));

        // should not include the already fetched account since it has no storage updates
        assert!(!targets.contains_key(fetched_addr));
        // other accounts should still be included
        assert_eq!(targets.len(), state.accounts.len() - 1);
    }

    #[test]
    fn test_get_proof_targets_filter_already_fetched_storage() {
        let state = create_get_proof_targets_state();
        let mut fetched = MultiProofTargets::default();

        // mark one storage slot as already fetched
        let (addr, storage) = state.storages.iter().next().unwrap();
        let mut fetched_slots = HashSet::default();
        let fetched_slot = *storage.storage.keys().next().unwrap();
        fetched_slots.insert(fetched_slot);
        fetched.insert(*addr, fetched_slots);

        let targets = unwrap_legacy_targets(get_proof_targets(
            &state,
            &fetched,
            &MultiAddedRemovedKeys::new(),
            false,
        ));

        // should not include the already fetched storage slot
        let target_slots = &targets[addr];
        assert!(!target_slots.contains(&fetched_slot));
        assert_eq!(target_slots.len(), storage.storage.len() - 1);
    }

    #[test]
    fn test_get_proof_targets_empty_state() {
        let state = HashedPostState::default();
        let fetched = MultiProofTargets::default();

        let targets = unwrap_legacy_targets(get_proof_targets(
            &state,
            &fetched,
            &MultiAddedRemovedKeys::new(),
            false,
        ));

        assert!(targets.is_empty());
    }

    #[test]
    fn test_get_proof_targets_mixed_fetched_state() {
        let mut state = HashedPostState::default();
        let mut fetched = MultiProofTargets::default();

        let addr1 = B256::random();
        let addr2 = B256::random();
        let slot1 = B256::random();
        let slot2 = B256::random();

        state.accounts.insert(addr1, Some(Default::default()));
        state.accounts.insert(addr2, Some(Default::default()));

        let mut storage = HashedStorage::default();
        storage.storage.insert(slot1, U256::ZERO);
        storage.storage.insert(slot2, U256::from(1));
        state.storages.insert(addr1, storage);

        let mut fetched_slots = HashSet::default();
        fetched_slots.insert(slot1);
        fetched.insert(addr1, fetched_slots);

        let targets = unwrap_legacy_targets(get_proof_targets(
            &state,
            &fetched,
            &MultiAddedRemovedKeys::new(),
            false,
        ));

        assert!(targets.contains_key(&addr2));
        assert!(!targets[&addr1].contains(&slot1));
        assert!(targets[&addr1].contains(&slot2));
    }

    #[test]
    fn test_get_proof_targets_unmodified_account_with_storage() {
        let mut state = HashedPostState::default();
        let fetched = MultiProofTargets::default();

        let addr = B256::random();
        let slot1 = B256::random();
        let slot2 = B256::random();

        // don't add the account to state.accounts (simulating unmodified account)
        // but add storage updates for this account
        let mut storage = HashedStorage::default();
        storage.storage.insert(slot1, U256::from(1));
        storage.storage.insert(slot2, U256::from(2));
        state.storages.insert(addr, storage);

        assert!(!state.accounts.contains_key(&addr));
        assert!(!fetched.contains_key(&addr));

        let targets = unwrap_legacy_targets(get_proof_targets(
            &state,
            &fetched,
            &MultiAddedRemovedKeys::new(),
            false,
        ));

        // verify that we still get the storage slots for the unmodified account
        assert!(targets.contains_key(&addr));

        let target_slots = &targets[&addr];
        assert_eq!(target_slots.len(), 2);
        assert!(target_slots.contains(&slot1));
        assert!(target_slots.contains(&slot2));
    }

    #[test]
    fn test_get_proof_targets_with_removed_storage_keys() {
        let mut state = HashedPostState::default();
        let mut fetched = MultiProofTargets::default();
        let mut multi_added_removed_keys = MultiAddedRemovedKeys::new();

        let addr = B256::random();
        let slot1 = B256::random();
        let slot2 = B256::random();

        // add account to state
        state.accounts.insert(addr, Some(Default::default()));

        // add storage updates
        let mut storage = HashedStorage::default();
        storage.storage.insert(slot1, U256::from(100));
        storage.storage.insert(slot2, U256::from(200));
        state.storages.insert(addr, storage);

        // mark slot1 as already fetched
        let mut fetched_slots = HashSet::default();
        fetched_slots.insert(slot1);
        fetched.insert(addr, fetched_slots);

        // update multi_added_removed_keys to mark slot1 as removed
        let mut removed_state = HashedPostState::default();
        let mut removed_storage = HashedStorage::default();
        removed_storage.storage.insert(slot1, U256::ZERO); // U256::ZERO marks as removed
        removed_state.storages.insert(addr, removed_storage);
        multi_added_removed_keys.update_with_state(&removed_state);

        let targets = unwrap_legacy_targets(get_proof_targets(
            &state,
            &fetched,
            &multi_added_removed_keys,
            false,
        ));

        // slot1 should be included despite being fetched, because it's marked as removed
        assert!(targets.contains_key(&addr));
        let target_slots = &targets[&addr];
        assert_eq!(target_slots.len(), 2);
        assert!(target_slots.contains(&slot1)); // included because it's removed
        assert!(target_slots.contains(&slot2)); // included because it's not fetched
    }

    #[test]
    fn test_get_proof_targets_with_wiped_storage() {
        let mut state = HashedPostState::default();
        let fetched = MultiProofTargets::default();
        let multi_added_removed_keys = MultiAddedRemovedKeys::new();

        let addr = B256::random();
        let slot1 = B256::random();

        // add account to state
        state.accounts.insert(addr, Some(Default::default()));

        // add wiped storage
        let mut storage = HashedStorage::new(true);
        storage.storage.insert(slot1, U256::from(100));
        state.storages.insert(addr, storage);

        let targets = unwrap_legacy_targets(get_proof_targets(
            &state,
            &fetched,
            &multi_added_removed_keys,
            false,
        ));

        // account should be included because storage is wiped and account wasn't fetched
        assert!(targets.contains_key(&addr));
        let target_slots = &targets[&addr];
        assert_eq!(target_slots.len(), 1);
        assert!(target_slots.contains(&slot1));
    }

    #[test]
    fn test_get_proof_targets_removed_keys_not_in_state_update() {
        let mut state = HashedPostState::default();
        let mut fetched = MultiProofTargets::default();
        let mut multi_added_removed_keys = MultiAddedRemovedKeys::new();

        let addr = B256::random();
        let slot1 = B256::random();
        let slot2 = B256::random();
        let slot3 = B256::random();

        // add account to state
        state.accounts.insert(addr, Some(Default::default()));

        // add storage updates for slot1 and slot2 only
        let mut storage = HashedStorage::default();
        storage.storage.insert(slot1, U256::from(100));
        storage.storage.insert(slot2, U256::from(200));
        state.storages.insert(addr, storage);

        // mark all slots as already fetched
        let mut fetched_slots = HashSet::default();
        fetched_slots.insert(slot1);
        fetched_slots.insert(slot2);
        fetched_slots.insert(slot3); // slot3 is fetched but not in state update
        fetched.insert(addr, fetched_slots);

        // mark slot3 as removed (even though it's not in the state update)
        let mut removed_state = HashedPostState::default();
        let mut removed_storage = HashedStorage::default();
        removed_storage.storage.insert(slot3, U256::ZERO);
        removed_state.storages.insert(addr, removed_storage);
        multi_added_removed_keys.update_with_state(&removed_state);

        let targets = unwrap_legacy_targets(get_proof_targets(
            &state,
            &fetched,
            &multi_added_removed_keys,
            false,
        ));

        // only slots in the state update can be included, so slot3 should not appear
        assert!(!targets.contains_key(&addr));
    }

    /// Verifies that consecutive prefetch proof messages are batched together.
    #[test]
    fn test_prefetch_proofs_batching() {
        let test_provider_factory = create_test_provider_factory();
        let mut task = create_test_state_root_task(test_provider_factory);

        // send multiple messages
        let addr1 = B256::random();
        let addr2 = B256::random();
        let addr3 = B256::random();

        let mut targets1 = MultiProofTargets::default();
        targets1.insert(addr1, HashSet::default());

        let mut targets2 = MultiProofTargets::default();
        targets2.insert(addr2, HashSet::default());

        let mut targets3 = MultiProofTargets::default();
        targets3.insert(addr3, HashSet::default());

        let tx = task.tx.clone();
        tx.send(MultiProofMessage::PrefetchProofs(VersionedMultiProofTargets::Legacy(targets1)))
            .unwrap();
        tx.send(MultiProofMessage::PrefetchProofs(VersionedMultiProofTargets::Legacy(targets2)))
            .unwrap();
        tx.send(MultiProofMessage::PrefetchProofs(VersionedMultiProofTargets::Legacy(targets3)))
            .unwrap();

        let proofs_requested =
            if let Ok(MultiProofMessage::PrefetchProofs(targets)) = task.rx.recv() {
                // simulate the batching logic
                let mut merged_targets = targets;
                let mut num_batched = 1;
                while let Ok(MultiProofMessage::PrefetchProofs(next_targets)) = task.rx.try_recv() {
                    merged_targets.extend(next_targets);
                    num_batched += 1;
                }

                assert_eq!(num_batched, 3);
                assert_eq!(merged_targets.len(), 3);
                let legacy_targets = unwrap_legacy_targets(merged_targets);
                assert!(legacy_targets.contains_key(&addr1));
                assert!(legacy_targets.contains_key(&addr2));
                assert!(legacy_targets.contains_key(&addr3));

                task.on_prefetch_proof(VersionedMultiProofTargets::Legacy(legacy_targets))
            } else {
                panic!("Expected PrefetchProofs message");
            };

        assert_eq!(proofs_requested, 1);
    }

    /// Verifies that different message types arriving mid-batch are not lost and preserve order.
    #[test]
    fn test_batching_preserves_ordering_with_different_message_type() {
        use alloy_evm::block::StateChangeSource;
        use revm_state::Account;

        let test_provider_factory = create_test_provider_factory();
        let task = create_test_state_root_task(test_provider_factory);

        let addr1 = B256::random();
        let addr2 = B256::random();
        let addr3 = B256::random();
        let state_addr1 = alloy_primitives::Address::random();
        let state_addr2 = alloy_primitives::Address::random();

        // Create PrefetchProofs targets
        let mut targets1 = MultiProofTargets::default();
        targets1.insert(addr1, HashSet::default());

        let mut targets2 = MultiProofTargets::default();
        targets2.insert(addr2, HashSet::default());

        let mut targets3 = MultiProofTargets::default();
        targets3.insert(addr3, HashSet::default());

        // Create StateUpdate 1
        let mut state_update1 = EvmState::default();
        state_update1.insert(
            state_addr1,
            Account {
                info: revm_state::AccountInfo {
                    balance: U256::from(100),
                    nonce: 1,
                    code_hash: Default::default(),
                    code: Default::default(),
                    account_id: None,
                },
                original_info: Box::new(revm_state::AccountInfo::default()),
                transaction_id: Default::default(),
                storage: Default::default(),
                status: revm_state::AccountStatus::Touched,
            },
        );

        // Create StateUpdate 2
        let mut state_update2 = EvmState::default();
        state_update2.insert(
            state_addr2,
            Account {
                info: revm_state::AccountInfo {
                    balance: U256::from(200),
                    nonce: 2,
                    code_hash: Default::default(),
                    code: Default::default(),
                    account_id: None,
                },
                original_info: Box::new(revm_state::AccountInfo::default()),
                transaction_id: Default::default(),
                storage: Default::default(),
                status: revm_state::AccountStatus::Touched,
            },
        );

        let source = StateChangeSource::Transaction(42);

        // Queue: [PrefetchProofs1, PrefetchProofs2, StateUpdate1, StateUpdate2, PrefetchProofs3]
        let tx = task.tx.clone();
        tx.send(MultiProofMessage::PrefetchProofs(VersionedMultiProofTargets::Legacy(targets1)))
            .unwrap();
        tx.send(MultiProofMessage::PrefetchProofs(VersionedMultiProofTargets::Legacy(targets2)))
            .unwrap();
        tx.send(MultiProofMessage::StateUpdate(source.into(), state_update1)).unwrap();
        tx.send(MultiProofMessage::StateUpdate(source.into(), state_update2)).unwrap();
        tx.send(MultiProofMessage::PrefetchProofs(VersionedMultiProofTargets::Legacy(
            targets3.clone(),
        )))
        .unwrap();

        // Step 1: Receive and batch PrefetchProofs (should get targets1 + targets2)
        let mut pending_msg: Option<MultiProofMessage> = None;
        if let Ok(MultiProofMessage::PrefetchProofs(targets)) = task.rx.recv() {
            let mut merged_targets = targets;
            let mut num_batched = 1;

            loop {
                match task.rx.try_recv() {
                    Ok(MultiProofMessage::PrefetchProofs(next_targets)) => {
                        merged_targets.extend(next_targets);
                        num_batched += 1;
                    }
                    Ok(other_msg) => {
                        // Store locally to preserve ordering (the fix)
                        pending_msg = Some(other_msg);
                        break;
                    }
                    Err(_) => break,
                }
            }

            // Should have batched exactly 2 PrefetchProofs (not 3!)
            assert_eq!(num_batched, 2, "Should batch only until different message type");
            assert_eq!(merged_targets.len(), 2);
            let legacy_targets = unwrap_legacy_targets(merged_targets);
            assert!(legacy_targets.contains_key(&addr1));
            assert!(legacy_targets.contains_key(&addr2));
            assert!(!legacy_targets.contains_key(&addr3), "addr3 should NOT be in first batch");
        } else {
            panic!("Expected PrefetchProofs message");
        }

        // Step 2: The pending message should be StateUpdate1 (preserved ordering)
        match pending_msg {
            Some(MultiProofMessage::StateUpdate(_src, update)) => {
                assert!(update.contains_key(&state_addr1), "Should be first StateUpdate");
            }
            _ => panic!("StateUpdate1 was lost or reordered! The ordering fix is broken."),
        }

        // Step 3: Next in channel should be StateUpdate2
        match task.rx.try_recv() {
            Ok(MultiProofMessage::StateUpdate(_src, update)) => {
                assert!(update.contains_key(&state_addr2), "Should be second StateUpdate");
            }
            _ => panic!("StateUpdate2 was lost!"),
        }

        // Step 4: Next in channel should be PrefetchProofs3
        match task.rx.try_recv() {
            Ok(MultiProofMessage::PrefetchProofs(targets)) => {
                assert_eq!(targets.len(), 1);
                let legacy_targets = unwrap_legacy_targets(targets);
                assert!(legacy_targets.contains_key(&addr3));
            }
            _ => panic!("PrefetchProofs3 was lost!"),
        }
    }

    /// Verifies that a pending message is processed before the next loop iteration (ordering).
    #[test]
    fn test_pending_message_processed_before_next_iteration() {
        use alloy_evm::block::StateChangeSource;
        use revm_state::Account;

        let test_provider_factory = create_test_provider_factory();
        let test_provider = create_cached_provider(test_provider_factory.clone());
        let mut task = create_test_state_root_task(test_provider_factory);

        // Queue: Prefetch1, StateUpdate, Prefetch2
        let prefetch_addr1 = B256::random();
        let prefetch_addr2 = B256::random();
        let mut prefetch1 = MultiProofTargets::default();
        prefetch1.insert(prefetch_addr1, HashSet::default());
        let mut prefetch2 = MultiProofTargets::default();
        prefetch2.insert(prefetch_addr2, HashSet::default());

        let state_addr = alloy_primitives::Address::random();
        let mut state_update = EvmState::default();
        state_update.insert(
            state_addr,
            Account {
                info: revm_state::AccountInfo {
                    balance: U256::from(42),
                    nonce: 1,
                    code_hash: Default::default(),
                    code: Default::default(),
                    account_id: None,
                },
                original_info: Box::new(revm_state::AccountInfo::default()),
                transaction_id: Default::default(),
                storage: Default::default(),
                status: revm_state::AccountStatus::Touched,
            },
        );

        let source = StateChangeSource::Transaction(99);

        let tx = task.tx.clone();
        tx.send(MultiProofMessage::PrefetchProofs(VersionedMultiProofTargets::Legacy(prefetch1)))
            .unwrap();
        tx.send(MultiProofMessage::StateUpdate(source.into(), state_update)).unwrap();
        tx.send(MultiProofMessage::PrefetchProofs(VersionedMultiProofTargets::Legacy(
            prefetch2.clone(),
        )))
        .unwrap();

        let mut ctx = MultiproofBatchCtx::new(Instant::now());
        let mut batch_metrics = MultiproofBatchMetrics::default();

        // First message: Prefetch1 batches; StateUpdate becomes pending.
        let first = task.rx.recv().unwrap();
        assert!(matches!(first, MultiProofMessage::PrefetchProofs(_)));
        assert!(!task.process_multiproof_message(
            first,
            &mut ctx,
            &mut batch_metrics,
            &test_provider
        ));
        let pending = ctx.pending_msg.take().expect("pending message captured");
        assert!(matches!(pending, MultiProofMessage::StateUpdate(_, _)));

        // Pending message should be handled before the next select loop.
        // StateUpdate is processed directly without batching.
        assert!(!task.process_multiproof_message(
            pending,
            &mut ctx,
            &mut batch_metrics,
            &test_provider
        ));

        // Since StateUpdate doesn't batch, Prefetch2 remains in the channel (not in pending_msg).
        assert!(ctx.pending_msg.is_none());

        // Prefetch2 should still be in the channel.
        match task.rx.try_recv() {
            Ok(MultiProofMessage::PrefetchProofs(targets)) => {
                assert_eq!(targets.len(), 1);
                let legacy_targets = unwrap_legacy_targets(targets);
                assert!(legacy_targets.contains_key(&prefetch_addr2));
            }
            other => panic!("Expected PrefetchProofs2 in channel, got {:?}", other),
        }
    }

    /// Verifies that BAL messages are processed correctly and generate state updates.
    #[test]
    fn test_bal_message_processing() {
        let test_provider_factory = create_test_provider_factory();
        let test_provider = create_cached_provider(test_provider_factory.clone());
        let mut task = create_test_state_root_task(test_provider_factory);

        // Create a simple BAL with one account change
        let account_address = Address::random();
        let account_changes = AccountChanges {
            address: account_address,
            balance_changes: vec![BalanceChange::new(0, U256::from(1000))],
            nonce_changes: vec![],
            code_changes: vec![],
            storage_changes: vec![],
            storage_reads: vec![],
        };

        let bal = Arc::new(vec![account_changes]);

        let mut ctx = MultiproofBatchCtx::new(Instant::now());
        let mut batch_metrics = MultiproofBatchMetrics::default();

        let should_finish = task.process_multiproof_message(
            MultiProofMessage::BlockAccessList(bal),
            &mut ctx,
            &mut batch_metrics,
            &test_provider,
        );

        // BAL should mark updates as finished
        assert!(ctx.updates_finished_time.is_some());

        // Should have dispatched state update proofs
        assert!(batch_metrics.state_update_proofs_requested > 0);

        // Should need to wait for the results of those proofs to arrive
        assert!(!should_finish, "Should continue waiting for proofs");
    }
}
