//! Activates snap-downloaded state as the staged pipeline's starting point.
//!
//! The state is published at its pivot, the merkle stage rebuilds its trie, and only a matching
//! root accepts it.

use alloy_eips::BlockNumHash;
use reth_errors::RethError;
use reth_provider::{
    providers::ProviderNodeTypes, DBProvider, DatabaseProviderFactory, HeaderProvider,
    ProviderFactory, StageCheckpointReader, StageCheckpointWriter,
};
use reth_snap_sync::{SnapAttemptStore, SnapStateVerifier, SnapWrite};
use reth_stages::{stages::MerkleStage, ExecInput, PipelineError, Stage, StageId};
use tracing::info;

/// Activation of one attempt's downloaded state.
///
/// Every step commits on its own and repeats safely, so an interrupted activation runs again.
pub(crate) struct SnapActivation<N: ProviderNodeTypes> {
    factory: ProviderFactory<N>,
}

impl<N: ProviderNodeTypes> SnapActivation<N> {
    pub(crate) const fn new(factory: ProviderFactory<N>) -> Self {
        Self { factory }
    }

    /// Publishes the state downloaded under `write` at `pivot`, rebuilds its trie and accepts it,
    /// so the pipeline continues above the pivot.
    pub(super) fn activate(
        &self,
        write: SnapWrite,
        pivot: BlockNumHash,
    ) -> Result<Activation, PipelineError> {
        let provider = self.factory.database_provider_rw()?;
        // Forkchoice can reorg the pivot out while its state downloads, and publishing anchors
        // the node to it, so the attempt is dropped instead of published.
        let canonical =
            provider.sealed_header(pivot.number)?.is_some_and(|header| header.hash() == pivot.hash);
        if !canonical {
            provider
                .abandon_snap_attempt()
                .map_err(|error| PipelineError::Internal(RethError::other(error)))?;
            provider.commit()?;
            return Ok(Activation::PivotReorged)
        }
        provider
            .begin_snap_publish(pivot.number)
            .map_err(|error| PipelineError::Internal(RethError::other(error)))?;
        provider.commit()?;
        self.publish(pivot.number)?;
        info!(target: "sync::snap", pivot = pivot.number, "Snap state published; history below it is unavailable");

        self.rebuild_trie(pivot)?;

        let provider = self.factory.database_provider_rw()?;
        provider
            .verify_state_root(write)
            .map_err(|error| PipelineError::Internal(RethError::other(error)))?;
        provider.commit()?;
        Ok(Activation::Published)
    }

    /// Finishes a publish interrupted between the static file and database commits, which left
    /// anchored static files under older checkpoints. Runs before the consistency check, which
    /// would otherwise try to unwind the files below their anchor.
    pub(crate) fn resume_interrupted_publish(&self) -> Result<(), PipelineError> {
        let pending = self
            .factory
            .database_provider_ro()?
            .pending_snap_publish()
            .map_err(|error| PipelineError::Internal(RethError::other(error)))?;
        let Some(pivot) = pending else { return Ok(()) };
        info!(target: "sync::snap", pivot, "Resuming an interrupted snap state publish");
        self.publish(pivot)
    }

    // Anchors the static files at `pivot` and moves the checkpoints there. Repeats safely, since
    // nothing is appended above the pivot until its checkpoints commit.
    fn publish(&self, pivot: u64) -> Result<(), PipelineError> {
        let provider = self.factory.database_provider_rw()?;
        // History below the pivot was never downloaded, so it counts as pruned and the static
        // files start there.
        provider.anchor_pruned_static_files(pivot)?;
        provider
            .publish_snap_state(pivot)
            .map_err(|error| PipelineError::Internal(RethError::other(error)))?;
        provider.commit()?;
        Ok(())
    }

    // Rebuilds the trie from the downloaded state up to `pivot`, committing the stage's progress
    // in chunks so a restart resumes it. The stage checks the root against the pivot's header.
    fn rebuild_trie(&self, pivot: BlockNumHash) -> Result<(), PipelineError> {
        let mut stage = MerkleStage::default_execution();
        loop {
            let provider = self.factory.database_provider_rw()?;
            let checkpoint = provider.get_stage_checkpoint(StageId::MerkleExecute)?;
            let output =
                stage.execute(&provider, ExecInput { target: Some(pivot.number), checkpoint })?;
            provider.save_stage_checkpoint(StageId::MerkleExecute, output.checkpoint)?;
            provider.commit()?;
            if output.done {
                return Ok(())
            }
        }
    }
}

/// What an activation did with the downloaded state.
#[derive(Debug, Eq, PartialEq)]
pub(super) enum Activation {
    /// The state is published, its trie rebuilt and its root verified.
    Published,
    /// The pivot left the canonical chain, so the attempt was abandoned for a new one.
    PivotReorged,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::Header;
    use alloy_primitives::B256;
    use reth_provider::{
        test_utils::{create_test_provider_factory, MockNodeTypesWithDB},
        BlockWriter, MetadataWriter, StaticFileProviderFactory, StaticFileSegment,
        StaticFileWriter, StorageSettings, StorageSettingsCache,
    };
    use reth_snap_sync::SnapGeneration;
    use reth_stages::StageCheckpoint;

    const PIVOT: u64 = 1;

    // Headers through the pivot on storage v2.
    fn headers() -> ProviderFactory<MockNodeTypesWithDB> {
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

    // Headers through the pivot, with the state published at it.
    fn published() -> ProviderFactory<MockNodeTypesWithDB> {
        let factory = headers();
        let provider = factory.database_provider_rw().unwrap();
        provider.anchor_pruned_static_files(PIVOT).unwrap();
        provider.publish_snap_state(PIVOT).unwrap();
        provider.commit().unwrap();
        factory
    }

    #[test]
    fn the_block_after_the_pivot_appends_to_the_anchored_files() {
        let factory = published();
        let provider = factory.database_provider_rw().unwrap();

        provider.append_block_bodies(vec![(PIVOT + 1, Some(&Default::default()))]).unwrap();
        provider.commit().unwrap();
    }

    #[test]
    fn a_pivot_reorged_out_is_abandoned_instead_of_published() {
        let factory = published();
        let provider = factory.database_provider_rw().unwrap();
        let orphan = BlockNumHash::new(PIVOT, B256::repeat_byte(0xaa));
        let write = provider
            .start_snap_attempt(SnapGeneration::new(orphan, B256::repeat_byte(0xbb)))
            .unwrap();
        provider.commit().unwrap();

        // The canonical header at the pivot's number is a different block now.
        let activation = SnapActivation::new(factory.clone()).activate(write, orphan).unwrap();

        assert_eq!(activation, Activation::PivotReorged);
        let provider = factory.database_provider_ro().unwrap();
        assert!(provider.active_snap_write().unwrap().is_none());
    }

    #[test]
    fn a_publish_interrupted_before_its_checkpoints_resumes_at_startup() {
        let factory = headers();
        let provider = factory.database_provider_rw().unwrap();
        provider.begin_snap_publish(PIVOT).unwrap();
        provider.commit().unwrap();

        // The static files finalize, then the node stops before the database commits.
        let provider = factory.database_provider_rw().unwrap();
        provider.anchor_pruned_static_files(PIVOT).unwrap();
        provider.publish_snap_state(PIVOT).unwrap();
        factory.static_file_provider().finalize().unwrap();
        drop(provider);

        // Startup finishes the publish before checking consistency, which would otherwise unwind
        // the anchored files to the old checkpoints.
        SnapActivation::new(factory.clone()).resume_interrupted_publish().unwrap();
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
        assert_eq!(provider.pending_snap_publish().unwrap(), None);
        assert_eq!(
            provider.get_stage_checkpoint(StageId::Execution).unwrap(),
            Some(StageCheckpoint::new(PIVOT))
        );
    }
}
