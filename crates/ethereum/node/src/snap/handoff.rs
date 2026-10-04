//! Hands snap-downloaded state over to the staged pipeline.
//!
//! The merkle stage rebuilds the trie from the downloaded state, and only a matching root on a
//! still canonical pivot is published and accepted.

use alloy_eips::BlockNumHash;
use reth_errors::{ConsensusError, RethError};
use reth_provider::{
    providers::ProviderNodeTypes, DBProvider, DatabaseProviderFactory, HeaderProvider,
    MetadataProvider, ProviderFactory, ProviderResult, StageCheckpointReader,
    StageCheckpointWriter, StaticFileProviderFactory, StaticFileSegment,
};
use reth_snap_sync::{SnapAttemptStore, SnapStateVerifier, SnapWrite};
use reth_stages::{
    stages::MerkleStage, BlockErrorKind, ExecInput, PipelineError, Stage, StageError, StageId,
};
use reth_tracing::tracing::info;
use tokio_util::sync::CancellationToken;

/// Hands one attempt's downloaded state over to the staged pipeline.
///
/// Every step commits on its own and repeats safely, so an interrupted handoff runs again.
#[derive(Debug)]
pub struct SnapHandoff<N: ProviderNodeTypes> {
    // Database the state was downloaded into.
    factory: ProviderFactory<N>,
}

impl<N: ProviderNodeTypes> SnapHandoff<N> {
    /// Creates a handoff over the node's database.
    pub const fn new(factory: ProviderFactory<N>) -> Self {
        Self { factory }
    }

    /// Rebuilds the trie at `pivot` and checks its root, abandoning the attempt on a mismatch.
    /// Progress commits as it goes, and `stop` ends it early.
    pub fn rebuild(
        &self,
        pivot: BlockNumHash,
        stop: &CancellationToken,
    ) -> Result<(), PipelineError> {
        let rebuilt = self.rebuild_trie(pivot, stop);
        if matches!(
            &rebuilt,
            Err(PipelineError::Stage(StageError::Block {
                error: BlockErrorKind::Validation(ConsensusError::BodyStateRootDiff(_)),
                ..
            }))
        ) {
            self.abandon()?;
        }
        rebuilt
    }

    /// Publishes the state downloaded under `write` at `pivot` and accepts it, so the pipeline
    /// continues above the pivot. The pivot must still be canonical, and its trie is rebuilt
    /// first if [`Self::rebuild`] hasn't finished.
    pub fn hand_off(
        &self,
        write: SnapWrite,
        pivot: BlockNumHash,
    ) -> Result<HandoffOutcome, PipelineError> {
        if !self.is_canonical(pivot)? {
            self.abandon()?;
            return Ok(HandoffOutcome::PivotReorged)
        }

        // A trie already rebuilt to the pivot returns at once.
        self.rebuild(pivot, &CancellationToken::new())?;

        self.publish(pivot.number)?;
        info!(target: "sync::snap", pivot = pivot.number, "Snap state published; history below it is unavailable");

        let provider = self.factory.database_provider_rw()?;
        provider.verify_state_root(write).map_err(RethError::other)?;
        provider.commit()?;
        Ok(HandoffOutcome::Completed)
    }

    /// Finishes a publish that anchored the static files but stopped before its checkpoints
    /// committed. Runs before the consistency check, which would otherwise try to unwind the files
    /// below their anchor. Does nothing otherwise.
    pub fn resume_interrupted_publish(&self) -> Result<(), PipelineError> {
        let Some(pivot) = self.interrupted_publish()? else { return Ok(()) };
        info!(target: "sync::snap", pivot, "Resuming an interrupted snap state publish");
        self.publish(pivot)
    }

    // The pivot of a publish that anchored the static files but stopped before its checkpoints
    // committed: the attempt is unfinished, the files start at its pivot, and execution is below
    // it.
    fn interrupted_publish(&self) -> ProviderResult<Option<u64>> {
        let provider = self.factory.provider()?;
        let Some(attempt) = provider.snap_attempt()?.filter(|attempt| attempt.is_unfinished())
        else {
            return Ok(None)
        };
        let pivot = attempt.pivot().number;
        let static_files_start = self
            .factory
            .static_file_provider()
            .get_lowest_range_start(StaticFileSegment::Transactions);
        let executed =
            provider.get_stage_checkpoint(StageId::Execution)?.unwrap_or_default().block_number;
        Ok((static_files_start == Some(pivot) && executed < pivot).then_some(pivot))
    }

    // Forkchoice can reorg the pivot out while its state downloads, and publishing anchors the
    // node to it, so a pivot that is no longer canonical is dropped instead of published.
    fn is_canonical(&self, pivot: BlockNumHash) -> ProviderResult<bool> {
        Ok(self
            .factory
            .provider()?
            .sealed_header(pivot.number)?
            .is_some_and(|header| header.hash() == pivot.hash))
    }

    // Drops the attempt, so the next run starts a new one.
    fn abandon(&self) -> Result<(), PipelineError> {
        let provider = self.factory.database_provider_rw()?;
        provider.abandon_snap_attempt().map_err(RethError::other)?;
        provider.commit()?;
        Ok(())
    }

    // Anchors the static files at `pivot` and moves the checkpoints there. Repeats safely, since
    // nothing is appended above the pivot until its checkpoints commit.
    fn publish(&self, pivot: u64) -> Result<(), PipelineError> {
        let provider = self.factory.database_provider_rw()?;
        // History below the pivot was never downloaded, so it counts as pruned and the static
        // files start there.
        provider.anchor_pruned_static_files(pivot)?;
        provider.publish_snap_state(pivot).map_err(RethError::other)?;
        provider.commit()?;
        Ok(())
    }

    // Rebuilds the trie from the downloaded state up to `pivot`, committing the stage's progress
    // in chunks so a restart resumes it. The stage checks the root against the pivot's header.
    fn rebuild_trie(
        &self,
        pivot: BlockNumHash,
        stop: &CancellationToken,
    ) -> Result<(), PipelineError> {
        let mut stage = MerkleStage::default_execution();
        while !stop.is_cancelled() {
            let provider = self.factory.database_provider_rw()?;
            let checkpoint = provider.get_stage_checkpoint(StageId::MerkleExecute)?;
            let output =
                stage.execute(&provider, ExecInput { target: Some(pivot.number), checkpoint })?;
            provider.save_stage_checkpoint(StageId::MerkleExecute, output.checkpoint)?;
            provider.commit()?;
            if output.done {
                break
            }
        }
        Ok(())
    }
}

/// What a handoff did with the downloaded state.
#[must_use]
#[derive(Debug, Eq, PartialEq)]
pub enum HandoffOutcome {
    /// The state is published, its trie rebuilt and its root verified.
    Completed,
    /// The pivot left the canonical chain, so the attempt was abandoned for a new one.
    PivotReorged,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::Header;
    use alloy_primitives::B256;
    use reth_db::{tables, transaction::DbTxMut};
    use reth_primitives_traits::Account;
    use reth_provider::{
        test_utils::{create_test_provider_factory, MockNodeTypesWithDB},
        BlockWriter, MetadataWriter, StaticFileWriter, StorageSettings, StorageSettingsCache,
    };
    use reth_snap_sync::SnapGeneration;
    use reth_stages::StageCheckpoint;

    const PIVOT: u64 = 1;

    // Headers through the pivot on storage v2.
    fn with_headers() -> ProviderFactory<MockNodeTypesWithDB> {
        let factory = create_test_provider_factory();
        let provider = factory.database_provider_rw().unwrap();
        provider.write_storage_settings(StorageSettings::v2()).unwrap();
        provider.commit().unwrap();
        factory.set_storage_settings_cache(StorageSettings::v2());

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
        factory
    }

    // Headers through the pivot, with an unverified attempt downloading state at it.
    fn downloading() -> (ProviderFactory<MockNodeTypesWithDB>, SnapWrite, BlockNumHash) {
        let factory = with_headers();
        let provider = factory.database_provider_rw().unwrap();
        let pivot = provider.sealed_header(PIVOT).unwrap().unwrap().num_hash();
        let write =
            provider.start_snap_attempt(SnapGeneration::new(pivot, B256::repeat_byte(1))).unwrap();
        provider.commit().unwrap();
        (factory, write, pivot)
    }

    // Headers through the pivot, with the state published at it.
    fn published() -> ProviderFactory<MockNodeTypesWithDB> {
        let factory = with_headers();
        let provider = factory.database_provider_rw().unwrap();
        provider.anchor_pruned_static_files(PIVOT).unwrap();
        provider.publish_snap_state(PIVOT).unwrap();
        provider.commit().unwrap();
        factory
    }

    #[test]
    fn resuming_without_an_interrupted_publish_changes_nothing() {
        let (factory, ..) = downloading();

        SnapHandoff::new(factory.clone()).resume_interrupted_publish().unwrap();

        let provider = factory.database_provider_ro().unwrap();
        assert_eq!(provider.get_stage_checkpoint(StageId::Execution).unwrap(), None);
        assert_eq!(factory.check_consistency().unwrap(), (None, None));
    }

    #[test]
    fn the_block_after_the_pivot_appends_to_the_anchored_files() {
        let factory = published();
        let provider = factory.database_provider_rw().unwrap();

        // Appending fails unless the anchored segments start right after the pivot.
        provider.append_block_bodies(vec![(PIVOT + 1, Some(&Default::default()))]).unwrap();
        provider.commit().unwrap();
    }

    #[test]
    fn a_pivot_reorged_out_is_abandoned_instead_of_published() {
        let factory = with_headers();
        let provider = factory.database_provider_rw().unwrap();
        let orphan = BlockNumHash::new(PIVOT, B256::repeat_byte(0xaa));
        let write = provider
            .start_snap_attempt(SnapGeneration::new(orphan, B256::repeat_byte(0xbb)))
            .unwrap();
        provider.commit().unwrap();

        // The canonical header at the pivot's number is a different block now.
        let handoff = SnapHandoff::new(factory.clone()).hand_off(write, orphan).unwrap();

        assert_eq!(handoff, HandoffOutcome::PivotReorged);
        let provider = factory.database_provider_ro().unwrap();
        assert!(provider.active_snap_write().unwrap().is_none());
    }

    #[test]
    fn a_stopped_rebuild_leaves_the_trie_unbuilt() {
        let (factory, _, pivot) = downloading();
        let stop = CancellationToken::new();
        stop.cancel();

        SnapHandoff::new(factory.clone()).rebuild(pivot, &stop).unwrap();

        let provider = factory.database_provider_ro().unwrap();
        assert_eq!(provider.get_stage_checkpoint(StageId::MerkleExecute).unwrap(), None);
    }

    #[test]
    fn an_abandoned_attempt_is_not_an_interrupted_publish() {
        let (factory, ..) = downloading();
        let provider = factory.database_provider_rw().unwrap();
        provider.abandon_snap_attempt().unwrap();
        provider.anchor_pruned_static_files(PIVOT).unwrap();
        provider.commit().unwrap();

        SnapHandoff::new(factory.clone()).resume_interrupted_publish().unwrap();

        let provider = factory.database_provider_ro().unwrap();
        assert_eq!(provider.get_stage_checkpoint(StageId::Execution).unwrap(), None);
    }

    #[test]
    fn a_root_mismatch_abandons_the_attempt_before_publishing() {
        let (factory, write, pivot) = downloading();
        let provider = factory.database_provider_rw().unwrap();
        // State whose root differs from the empty root the pivot header commits to.
        provider
            .tx_ref()
            .put::<tables::HashedAccounts>(
                B256::repeat_byte(3),
                Account { nonce: 1, ..Default::default() },
            )
            .unwrap();
        provider.commit().unwrap();

        assert!(SnapHandoff::new(factory.clone()).hand_off(write, pivot).is_err());

        let provider = factory.database_provider_ro().unwrap();
        assert!(provider.active_snap_write().unwrap().is_none());
        assert_eq!(provider.get_stage_checkpoint(StageId::Execution).unwrap(), None);
        assert_eq!(
            factory.static_file_provider().get_lowest_range_start(StaticFileSegment::Transactions),
            None
        );
    }

    #[test]
    fn a_publish_interrupted_before_its_checkpoints_resumes_at_startup() {
        let (factory, ..) = downloading();

        // The static files finalize, then the node stops before the database commits.
        let provider = factory.database_provider_rw().unwrap();
        provider.anchor_pruned_static_files(PIVOT).unwrap();
        provider.publish_snap_state(PIVOT).unwrap();
        factory.static_file_provider().finalize().unwrap();
        drop(provider);

        // Startup finishes the publish before checking consistency, which would otherwise unwind
        // the anchored files to the old checkpoints.
        SnapHandoff::new(factory.clone()).resume_interrupted_publish().unwrap();

        assert_eq!(factory.check_consistency().unwrap(), (None, None));
        let static_files = factory.static_file_provider();
        for segment in StaticFileSegment::iter().filter(|segment| !segment.is_headers()) {
            assert_eq!(
                static_files.get_highest_static_file_block(segment),
                Some(PIVOT),
                "{segment}"
            );
        }
        let provider = factory.database_provider_ro().unwrap();
        assert_eq!(
            provider.get_stage_checkpoint(StageId::Execution).unwrap(),
            Some(StageCheckpoint::new(PIVOT))
        );
    }
}
