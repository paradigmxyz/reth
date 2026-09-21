//! Publishes snap-downloaded state as the staged pipeline's starting point.
//! History below the pivot was never downloaded, so it is marked pruned and anchors the static
//! files; the merkle rebuild and `Finish` are left to run once the state is checked.

use reth_db_api::{tables, transaction::DbTxMut};
use reth_provider::{
    DBProvider, ProviderResult, PruneCheckpointWriter, StageCheckpointWriter,
    StaticFileProviderFactory, StaticFileSegment, StaticFileWriter,
};
use reth_prune::{PruneCheckpoint, PruneMode, PruneSegment};
use reth_stages_api::{StageCheckpoint, StageId};
use tracing::info;

// Stages the downloaded state satisfies at the pivot. Headers were downloaded for real, the merkle
// stage still has to rebuild the trie, and `Finish` only advances once the state is verified.
const PUBLISHED_STAGES: [StageId; 11] = [
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
    StageId::Prune,
];

// Segments whose rows below the pivot a snap sync never produced.
const PRUNED_SEGMENTS: [PruneSegment; 6] = [
    PruneSegment::Bodies,
    PruneSegment::SenderRecovery,
    PruneSegment::TransactionLookup,
    PruneSegment::Receipts,
    PruneSegment::AccountHistory,
    PruneSegment::StorageHistory,
];

/// Moves every stage the downloaded state covers to `pivot`, records the history below it as
/// pruned and anchors the non-header static files there, so the pipeline continues at `pivot + 1`.
///
/// Repeatable until the caller commits, so an interrupted hand-off can run again.
pub(crate) fn publish_snap_state(
    provider: &(impl DBProvider<Tx: DbTxMut>
          + PruneCheckpointWriter
          + StageCheckpointWriter
          + StaticFileProviderFactory),
    pivot: u64,
) -> ProviderResult<()> {
    let static_files = provider.static_file_provider();
    for segment in StaticFileSegment::iter().filter(|segment| !segment.is_headers()) {
        static_files.delete_segment(segment)?;
        static_files.get_writer(pivot, segment)?.initialize_pruned_anchor(pivot)?;
    }
    // Bodies downloaded before the pivot moved allocated transaction numbers the empty
    // transaction segment no longer holds.
    provider.tx_ref().clear::<tables::BlockBodyIndices>()?;
    provider.tx_ref().clear::<tables::TransactionBlocks>()?;
    provider.tx_ref().clear::<tables::TransactionHashNumbers>()?;

    let checkpoint = StageCheckpoint::new(pivot);
    for stage in PUBLISHED_STAGES {
        provider.save_stage_checkpoint(stage, checkpoint)?;
    }
    // `before_inclusive` keeps the pivot itself, whose state the node has.
    let pruned = PruneCheckpoint {
        block_number: Some(pivot),
        tx_number: None,
        prune_mode: PruneMode::before_inclusive(pivot),
    };
    for segment in PRUNED_SEGMENTS {
        provider.save_prune_checkpoint(segment, pruned)?;
    }

    info!(target: "sync::snap", pivot, "Snap state published; history below it is unavailable");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::Header;
    use alloy_primitives::B256;
    use reth_provider::{
        test_utils::{create_test_provider_factory, MockNodeTypesWithDB},
        BlockWriter, DatabaseProviderFactory, ProviderFactory, PruneCheckpointReader,
        StageCheckpointReader,
    };

    const PIVOT: u64 = 1;

    // Headers through the pivot, with the state published at it.
    fn published() -> ProviderFactory<MockNodeTypesWithDB> {
        let factory = create_test_provider_factory();
        let static_files = factory.static_file_provider();
        let mut writer = static_files.latest_writer(StaticFileSegment::Headers).unwrap();
        let mut parent = B256::ZERO;
        for number in 0..=PIVOT {
            let header = Header { number, parent_hash: parent, ..Default::default() };
            let hash = header.hash_slow();
            writer.append_header(&header, &hash).unwrap();
            parent = hash;
        }
        writer.commit().unwrap();
        drop(writer);

        let provider = factory.database_provider_rw().unwrap();
        publish_snap_state(&provider, PIVOT).unwrap();
        provider.commit().unwrap();
        factory
    }

    #[test]
    fn covered_stages_move_to_the_pivot_while_the_trie_and_finish_wait() {
        let provider = published().database_provider_ro().unwrap();

        for stage in PUBLISHED_STAGES {
            let checkpoint = provider.get_stage_checkpoint(stage).unwrap();
            assert_eq!(checkpoint.map(|it| it.block_number), Some(PIVOT), "{stage}");
        }
        for stage in [StageId::Headers, StageId::MerkleExecute, StageId::Finish] {
            assert_eq!(provider.get_stage_checkpoint(stage).unwrap(), None, "{stage}");
        }
    }

    #[test]
    fn history_below_the_pivot_is_recorded_as_pruned() {
        let provider = published().database_provider_ro().unwrap();

        for segment in PRUNED_SEGMENTS {
            let checkpoint = provider.get_prune_checkpoint(segment).unwrap().unwrap();
            assert_eq!(checkpoint.block_number, Some(PIVOT));
            assert_eq!(checkpoint.prune_mode, PruneMode::before_inclusive(PIVOT));
        }
    }

    #[test]
    fn the_block_after_the_pivot_appends_to_the_anchored_files() {
        let factory = published();
        let provider = factory.database_provider_rw().unwrap();

        provider.append_block_bodies(vec![(PIVOT + 1, Some(&Default::default()))]).unwrap();
        provider.commit().unwrap();
    }
}
