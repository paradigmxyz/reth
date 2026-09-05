//! Publishes a completed generation as the staged pipeline's starting point.
//!
//! Bodies, receipts, senders, change sets and history indexes below the pivot were never
//! downloaded, so they are declared pruned rather than synced. History that is not available
//! locally is already representable: `earliest_history_height` follows the lowest static file
//! block, stages consult prune checkpoints before assuming a range exists, and the engine publishes
//! the served range to peers. Claiming the range was synced instead would fail the static file
//! consistency check, or answer historical queries from data that is not there.

use crate::{error::db_error, SnapStateStore, SnapSyncError};
use reth_provider::DatabaseProviderFactory;
use reth_prune_types::{PruneCheckpoint, PruneMode, PruneSegment};
use reth_stages_types::{StageCheckpoint, StageId};
use reth_storage_api::{PruneCheckpointWriter, StageCheckpointReader, StageCheckpointWriter};
use tracing::info;

// Stages a snap sync satisfies beyond those any externally supplied state covers: nothing below
// the pivot was downloaded, and the pipeline reports overall progress from `Finish`. Headers stay
// out, since they really were downloaded and the pipeline owns that checkpoint.
const EXTRA_STATE_STAGES: [StageId; 4] =
    [StageId::Bodies, StageId::SenderRecovery, StageId::TransactionLookup, StageId::Finish];

// Segments whose pre-pivot rows a snap sync never produced.
const PRUNED_SEGMENTS: [PruneSegment; 6] = [
    PruneSegment::Bodies,
    PruneSegment::SenderRecovery,
    PruneSegment::TransactionLookup,
    PruneSegment::Receipts,
    PruneSegment::AccountHistory,
    PruneSegment::StorageHistory,
];

/// Hands a finished generation to the staged pipeline.
#[derive(Debug)]
pub struct SnapPipelineHandoff<'a, F> {
    // The completed generation is read back through the store that wrote it.
    store: SnapStateStore<'a, F>,
}

impl<'a, F> SnapPipelineHandoff<'a, F> {
    /// Creates a handoff without opening a database transaction.
    pub const fn new(factory: &'a F) -> Self {
        Self { store: SnapStateStore::new(factory) }
    }

    /// Returns the block the pipeline was handed, if this node was snap synced.
    ///
    /// Nothing below it can be re-executed: the change sets a rewind replays were never
    /// downloaded, so an unwind must treat it as a floor and require a fresh snap sync.
    ///
    /// `geth` stops at its own pivot for the same reason, returning genesis as the new head
    /// rather than rewinding towards it when no persistent state exists below.
    pub fn published_block(&self) -> Result<Option<u64>, SnapSyncError>
    where
        F: DatabaseProviderFactory<Provider: StageCheckpointReader>,
    {
        self.store.completed_block()
    }
}

/// Publishes the state at `block_number` as the frontier every state stage starts from.
///
/// The pipeline resumes at `block_number + 1`, and everything below it is recorded as pruned so no
/// stage looks for rows that were never downloaded. The prune checkpoints double as the unwind
/// floor: `PruneModes::ensure_unwind_target_unpruned` refuses to rewind past them.
///
/// Writes through `provider` without committing, so the caller can make publication atomic with
/// whatever else accepts the state.
pub(crate) fn publish_state_snapshot(
    provider: &(impl PruneCheckpointWriter + StageCheckpointWriter),
    block_number: u64,
) -> Result<(), SnapSyncError> {
    let checkpoint = StageCheckpoint::new(block_number);
    for stage in StageId::STATE_REQUIRED.into_iter().chain(EXTRA_STATE_STAGES) {
        provider.save_stage_checkpoint(stage, checkpoint).map_err(db_error)?;
    }

    // `before_inclusive` keeps the pivot itself, whose state the node does have.
    let pruned = PruneCheckpoint {
        block_number: Some(block_number),
        tx_number: None,
        prune_mode: PruneMode::before_inclusive(block_number),
    };
    for segment in PRUNED_SEGMENTS {
        provider.save_prune_checkpoint(segment, pruned).map_err(db_error)?;
    }

    info!(
        target: "snap::handoff",
        block_number,
        "Snap state published; history below it is unavailable"
    );
    Ok(())
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

    // Publication rides the generation's own commit, so a finished snap sync has already
    // published its frontier.
    #[test]
    fn published_state_moves_every_state_stage_to_the_pivot() {
        let (factory, pivot) = snap_synced_factory();
        let handoff = SnapPipelineHandoff::new(&factory);

        let provider = factory.database_provider_ro().unwrap();
        for stage in StageId::STATE_REQUIRED.into_iter().chain(EXTRA_STATE_STAGES) {
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

        let provider = factory.database_provider_ro().unwrap();
        for segment in PRUNED_SEGMENTS {
            let checkpoint = provider.get_prune_checkpoint(segment).unwrap().unwrap();
            assert_eq!(checkpoint.block_number, Some(pivot));
            assert_eq!(checkpoint.prune_mode, PruneMode::before_inclusive(pivot));
        }
    }

    #[test]
    fn unfinished_state_is_not_published() {
        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(StorageSettings::v2());
        let generation = SnapGeneration::new(7, B256::repeat_byte(1), B256::repeat_byte(2));
        SnapStateStore::new(&factory).begin_generation(generation).unwrap();

        // Still downloading accounts, so the trie phase, and with it publication, is refused.
        let error = TrieGenerator::new(&factory).run(generation).unwrap_err();

        assert!(matches!(error, SnapSyncError::UnexpectedPhase { .. }));
        let provider = factory.database_provider_ro().unwrap();
        assert_eq!(provider.get_stage_checkpoint(StageId::Execution).unwrap(), None);
        assert_eq!(SnapPipelineHandoff::new(&factory).published_block().unwrap(), None);
    }
}
