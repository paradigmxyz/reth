use crate::BlockHashReader;
use alloc::{string::String, vec::Vec};
use alloy_eips::BlockNumHash;
use alloy_primitives::BlockNumber;
use reth_stages_types::{StageCheckpoint, StageId};
use reth_storage_errors::provider::{ProviderError, ProviderResult};

/// The trait for fetching stage checkpoint related data.
#[auto_impl::auto_impl(&, Arc)]
pub trait StageCheckpointReader: Send {
    /// Fetch the checkpoint for the given stage.
    fn get_stage_checkpoint(&self, id: StageId) -> ProviderResult<Option<StageCheckpoint>>;

    /// Get stage checkpoint progress.
    fn get_stage_checkpoint_progress(&self, id: StageId) -> ProviderResult<Option<Vec<u8>>>;

    /// Reads all stage checkpoints and returns a list with the name of the stage and the checkpoint
    /// data.
    fn get_all_checkpoints(&self) -> ProviderResult<Vec<(String, StageCheckpoint)>>;
}

/// Reads the last block that survives a restart.
#[auto_impl::auto_impl(&, Arc)]
pub trait DurableBlockReader: StageCheckpointReader + BlockHashReader {
    /// Returns the number and hash of the last block that is still canonical in the database after
    /// a restart.
    ///
    /// This can be behind
    /// [`BlockNumReader::last_block_number`](crate::BlockNumReader::last_block_number): static
    /// files are written before the database transaction that commits the same blocks, and blocks
    /// above the partial state/trie frontier of the `Finish` stage checkpoint are unwound on
    /// startup. Returns genesis if there is no `Finish` checkpoint.
    ///
    /// The hash is the database's block at that height, never an in-memory canonical block, so it
    /// identifies the block a restart keeps even while an in-memory reorg below it is not yet
    /// persisted. Providers that serve in-memory blocks override this to read the database.
    ///
    /// The block only survives a host crash or power loss when the database syncs every commit to
    /// disk. With `--db.sync-mode safe-no-sync`, it survives a process crash, but MDBX may
    /// discard the latest commits when the database is reopened after a host failure.
    fn last_durable_block_num_hash(&self) -> ProviderResult<BlockNumHash> {
        // The Finish checkpoint is written in the same transaction as the blocks it covers.
        let number = self
            .get_stage_checkpoint(StageId::Finish)?
            .map(|checkpoint| {
                checkpoint
                    .finish_stage_checkpoint()
                    .and_then(|finish| finish.partial_state_trie())
                    .unwrap_or(checkpoint.block_number)
            })
            .unwrap_or_default();
        let hash = self.block_hash(number)?.ok_or(ProviderError::HeaderNotFound(number.into()))?;
        Ok(BlockNumHash::new(number, hash))
    }
}

/// The trait for updating stage checkpoint related data.
#[auto_impl::auto_impl(&, Arc)]
pub trait StageCheckpointWriter {
    /// Save stage checkpoint.
    fn save_stage_checkpoint(&self, id: StageId, checkpoint: StageCheckpoint)
        -> ProviderResult<()>;

    /// Save stage checkpoint progress.
    fn save_stage_checkpoint_progress(
        &self,
        id: StageId,
        checkpoint: Vec<u8>,
    ) -> ProviderResult<()>;

    /// Update all pipeline sync stage progress.
    fn update_pipeline_stages(
        &self,
        block_number: BlockNumber,
        drop_stage_checkpoint: bool,
    ) -> ProviderResult<()>;
}
