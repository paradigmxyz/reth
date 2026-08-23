//! Publishes a completed generation as the staged pipeline's starting point.
//!
//! Bodies, receipts, senders, change sets and history indexes below the pivot were never
//! downloaded, so they are declared pruned rather than synced. Reth already represents history
//! that is not available locally: `earliest_history_height` follows the lowest static file block,
//! stages consult prune checkpoints before assuming a range exists, and the engine publishes the
//! served range to peers. Claiming the range was synced instead would fail the static file
//! consistency check, or answer historical queries from data that is not there.

use crate::{error::db_error, SnapStateStore, SnapSyncError};
use reth_provider::DatabaseProviderFactory;
use reth_prune_types::{PruneCheckpoint, PruneMode, PruneSegment};
use reth_stages_types::{StageCheckpoint, StageId};
use reth_storage_api::{
    DBProvider, PruneCheckpointWriter, StageCheckpointReader, StageCheckpointWriter,
};
use tracing::info;

// Stages whose work the downloaded state stands in for. Headers are excluded: they were really
// downloaded, and the pipeline owns that checkpoint.
const STATE_STAGES: [StageId; 10] = [
    StageId::Bodies,
    StageId::SenderRecovery,
    StageId::Execution,
    StageId::PruneSenderRecovery,
    StageId::MerkleUnwind,
    StageId::AccountHashing,
    StageId::StorageHashing,
    StageId::TransactionLookup,
    StageId::IndexStorageHistory,
    StageId::IndexAccountHistory,
];

// Segments whose pre-pivot rows a snap sync never produced.
const PRUNED_SEGMENTS: [PruneSegment; 5] = [
    PruneSegment::SenderRecovery,
    PruneSegment::TransactionLookup,
    PruneSegment::Receipts,
    PruneSegment::AccountHistory,
    PruneSegment::StorageHistory,
];

/// Hands a finished generation to the staged pipeline.
#[derive(Debug)]
pub struct SnapPipelineHandoff<'a, F> {
    // Checkpoint reads and writes observe the same provider factory.
    factory: &'a F,
    // The completed generation is read back through the store that wrote it.
    store: SnapStateStore<'a, F>,
}

impl<'a, F> SnapPipelineHandoff<'a, F> {
    /// Creates a handoff without opening a database transaction.
    pub const fn new(factory: &'a F) -> Self {
        Self { factory, store: SnapStateStore::new(factory) }
    }

    /// Publishes the state at `block_number` as the frontier every state stage starts from.
    ///
    /// The pipeline resumes at `block_number + 1`, and everything below it is recorded as pruned
    /// so no stage looks for rows that were never downloaded.
    ///
    /// # Errors
    ///
    /// Returns [`SnapSyncError::IncompleteState`] unless a generation was accepted at exactly
    /// `block_number`, since an unfinished generation's state is not a valid frontier.
    pub fn commit(&self, block_number: u64) -> Result<(), SnapSyncError>
    where
        F: DatabaseProviderFactory,
        F::ProviderRW:
            DBProvider + PruneCheckpointWriter + StageCheckpointReader + StageCheckpointWriter,
    {
        let provider = self.factory.database_provider_rw().map_err(db_error)?;
        let completed = SnapStateStore::<F>::completed_block_in(&provider)?;
        if completed != Some(block_number) {
            return Err(SnapSyncError::IncompleteState { expected: block_number, actual: completed })
        }

        let checkpoint = StageCheckpoint::new(block_number);
        for stage in STATE_STAGES {
            provider.save_stage_checkpoint(stage, checkpoint).map_err(db_error)?;
        }
        // The pipeline reports overall progress from the last stage, so it must agree.
        provider.save_stage_checkpoint(StageId::Finish, checkpoint).map_err(db_error)?;

        // `Before` keeps the pivot itself, whose state and receipts the node does have.
        let pruned = PruneCheckpoint {
            block_number: Some(block_number),
            tx_number: None,
            prune_mode: PruneMode::Before(block_number.saturating_add(1)),
        };
        for segment in PRUNED_SEGMENTS {
            provider.save_prune_checkpoint(segment, pruned).map_err(db_error)?;
        }
        provider.commit().map_err(db_error)?;

        info!(
            target: "snap::handoff",
            block_number,
            "Snap state published; history below it is unavailable"
        );
        Ok(())
    }

    /// Returns the block the pipeline was handed, if this node was snap synced.
    pub fn published_block(&self) -> Result<Option<u64>, SnapSyncError>
    where
        F: DatabaseProviderFactory<Provider: StageCheckpointReader>,
    {
        self.store.completed_block()
    }

    /// Checks that a rewind stops at or above the published state.
    ///
    /// Nothing below that block can be re-executed: the change sets a rewind replays were never
    /// downloaded, so a node that rewinds through it has no state to fall back on and has to snap
    /// sync again. Geth takes the same position on its own pivot, stopping the rewind rather than
    /// walking towards genesis: *"If pivot block is reached, return the genesis block as the new
    /// chain head. Theoretically there must be a persistent state before or at the pivot block,
    /// prevent endless rewinding towards the genesis just in case."*
    ///
    /// # Errors
    ///
    /// Returns [`SnapSyncError::BelowStateFloor`] when `target` is below the published block. The
    /// caller should treat that as needing a fresh snap sync, not as a recoverable unwind.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use reth_snap_sync::{SnapPipelineHandoff, SnapSyncError};
    ///
    /// # fn example<F>(factory: &F) -> Result<(), SnapSyncError>
    /// # where
    /// #     F: reth_provider::DatabaseProviderFactory<
    /// #         Provider: reth_storage_api::StageCheckpointReader,
    /// #     >,
    /// # {
    /// let handoff = SnapPipelineHandoff::new(factory);
    /// if let Err(SnapSyncError::BelowStateFloor { floor, .. }) = handoff.ensure_rewind_target(10) {
    ///     println!("resync required, snap state starts at {floor}");
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub fn ensure_rewind_target(&self, target: u64) -> Result<(), SnapSyncError>
    where
        F: DatabaseProviderFactory<Provider: StageCheckpointReader>,
    {
        let Some(floor) = self.published_block()? else { return Ok(()) };
        if target < floor {
            return Err(SnapSyncError::BelowStateFloor { target, floor })
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AccountRangeProgress, SnapGeneration, TrieGenerator};
    use alloy_consensus::Header;
    use alloy_primitives::{B256, KECCAK256_EMPTY, U256};
    use reth_primitives_traits::Account;
    use reth_provider::{
        test_utils::{create_test_provider_factory, MockNodeTypesWithDB},
        ProviderFactory, StaticFileProviderFactory, StaticFileWriter,
    };
    use reth_static_file_types::StaticFileSegment;
    use reth_storage_api::{PruneCheckpointReader, StorageSettings, StorageSettingsCache};
    use reth_trie_common::{HashBuilder, HashedPostState, Nibbles, TrieAccount, EMPTY_ROOT_HASH};

    // Runs a complete generation so the handoff has an accepted state frontier to publish.
    fn snap_synced_factory() -> (ProviderFactory<MockNodeTypesWithDB>, u64) {
        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(StorageSettings::v2());
        let account_hash = B256::repeat_byte(0x11);
        let account = TrieAccount {
            nonce: 3,
            balance: U256::from(4),
            storage_root: EMPTY_ROOT_HASH,
            code_hash: KECCAK256_EMPTY,
        };
        let mut builder = HashBuilder::default();
        builder.add_leaf(Nibbles::unpack(account_hash), &alloy_rlp::encode(account));
        let state_root = builder.root();
        let genesis = Header::default();
        let genesis_hash = genesis.hash_slow();
        let pivot =
            Header { number: 1, parent_hash: genesis_hash, state_root, ..Default::default() };
        let pivot_hash = pivot.hash_slow();
        let static_files = factory.static_file_provider();
        let mut writer = static_files.latest_writer(StaticFileSegment::Headers).unwrap();
        writer.append_header(&genesis, &genesis_hash).unwrap();
        writer.append_header(&pivot, &pivot_hash).unwrap();
        writer.commit().unwrap();
        drop(writer);
        drop(static_files);

        let store = SnapStateStore::new(&factory);
        let generation = SnapGeneration::new(1, pivot_hash, state_root);
        store.begin_generation(generation).unwrap();
        let generation = store
            .commit_account_range(
                generation,
                HashedPostState::default()
                    .with_accounts([(account_hash, Some(Account::from(account)))]),
                Vec::new(),
                AccountRangeProgress::Complete,
            )
            .unwrap();
        let generation = store.complete_block_access_lists(generation).unwrap();
        TrieGenerator::new(&factory).run(generation).unwrap();
        (factory, generation.target_block)
    }

    #[test]
    fn published_state_moves_every_state_stage_to_the_pivot() {
        let (factory, pivot) = snap_synced_factory();
        let handoff = SnapPipelineHandoff::new(&factory);

        handoff.commit(pivot).unwrap();

        let provider = factory.database_provider_ro().unwrap();
        for stage in STATE_STAGES.into_iter().chain([StageId::Finish]) {
            assert_eq!(
                provider.get_stage_checkpoint(stage).unwrap().map(|it| it.block_number),
                Some(pivot),
                "{stage} should start after the pivot"
            );
        }
        // Headers were downloaded for real, so the pipeline keeps owning that checkpoint.
        assert_eq!(provider.get_stage_checkpoint(StageId::Headers).unwrap(), None);
        assert_eq!(handoff.published_block().unwrap(), Some(pivot));
    }

    #[test]
    fn skipped_history_is_recorded_as_pruned() {
        let (factory, pivot) = snap_synced_factory();

        SnapPipelineHandoff::new(&factory).commit(pivot).unwrap();

        let provider = factory.database_provider_ro().unwrap();
        for segment in PRUNED_SEGMENTS {
            let checkpoint = provider.get_prune_checkpoint(segment).unwrap().unwrap();
            assert_eq!(checkpoint.block_number, Some(pivot));
            assert_eq!(checkpoint.prune_mode, PruneMode::Before(pivot + 1));
        }
    }

    #[test]
    fn rewind_stops_at_the_published_state() {
        let (factory, pivot) = snap_synced_factory();
        let handoff = SnapPipelineHandoff::new(&factory);
        handoff.commit(pivot).unwrap();

        assert!(handoff.ensure_rewind_target(pivot + 1).is_ok());
        assert!(handoff.ensure_rewind_target(pivot).is_ok());
        let error = handoff.ensure_rewind_target(pivot - 1).unwrap_err();

        assert!(matches!(
            error,
            SnapSyncError::BelowStateFloor { target, floor } if target == pivot - 1 && floor == pivot
        ));
    }

    #[test]
    fn a_node_that_never_snap_synced_has_no_floor() {
        let factory = create_test_provider_factory();

        assert!(SnapPipelineHandoff::new(&factory).ensure_rewind_target(0).is_ok());
    }

    #[test]
    fn unfinished_state_is_not_published() {
        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(StorageSettings::v2());
        let generation = SnapGeneration::new(7, B256::repeat_byte(1), B256::repeat_byte(2));
        SnapStateStore::new(&factory).begin_generation(generation).unwrap();

        let error = SnapPipelineHandoff::new(&factory).commit(7).unwrap_err();

        assert!(matches!(error, SnapSyncError::IncompleteState { expected: 7, actual: None }));
        let provider = factory.database_provider_ro().unwrap();
        assert_eq!(provider.get_stage_checkpoint(StageId::Execution).unwrap(), None);
    }
}
