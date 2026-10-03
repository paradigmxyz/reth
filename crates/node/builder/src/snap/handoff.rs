//! Activates snap-downloaded state as the staged pipeline's starting point.
//! History below the pivot was never downloaded, so it is marked pruned and anchors the static
//! files; the merkle stage then rebuilds the trie before the state is accepted.

use alloy_eips::BlockNumHash;
use reth_errors::RethError;
use reth_provider::{
    providers::ProviderNodeTypes, DBProvider, DatabaseProviderFactory, ProviderFactory,
    StageCheckpointReader, StageCheckpointWriter,
};
use reth_snap_sync::{SnapStateVerifier, SnapWrite};
use reth_stages::stages::MerkleStage;
use reth_stages_api::{ExecInput, PipelineError, Stage, StageId};
use tracing::info;

/// Publishes the state downloaded under `write` at `pivot`, rebuilds its trie and accepts it, so
/// the pipeline continues above the pivot.
///
/// Every step commits on its own and repeats safely, so an interrupted activation runs again.
pub(super) fn activate<N: ProviderNodeTypes>(
    factory: &ProviderFactory<N>,
    write: SnapWrite,
    pivot: BlockNumHash,
) -> Result<(), PipelineError> {
    let provider = factory.database_provider_rw()?;
    provider.anchor_pruned_static_files(pivot.number)?;
    provider
        .publish_snap_state(pivot.number)
        .map_err(|error| PipelineError::Internal(RethError::other(error)))?;
    provider.commit()?;
    info!(target: "sync::snap", pivot = pivot.number, "Snap state published; history below it is unavailable");
    rebuild_trie(factory, pivot)?;
    let provider = factory.database_provider_rw()?;
    provider
        .verify_state_root(write)
        .map_err(|error| PipelineError::Internal(RethError::other(error)))?;
    provider.commit()?;
    Ok(())
}

// Rebuilds the trie from the downloaded state up to `pivot`, committing the stage's progress in
// chunks so a restart resumes it. The stage checks the root against the pivot's header.
fn rebuild_trie<N: ProviderNodeTypes>(
    factory: &ProviderFactory<N>,
    pivot: BlockNumHash,
) -> Result<(), PipelineError> {
    let mut stage = MerkleStage::default_execution();
    loop {
        let provider = factory.database_provider_rw()?;
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

    const PIVOT: u64 = 1;

    // Headers through the pivot, with the state published at it.
    fn published() -> ProviderFactory<MockNodeTypesWithDB> {
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
}
