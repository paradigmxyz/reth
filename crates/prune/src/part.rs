use crate::PrunerError;
use reth_db::database::Database;
use reth_interfaces::RethResult;
use reth_primitives::{BlockNumber, PruneCheckpoint, PruneMode, PrunePart, TxNumber};
use reth_provider::{
    BlockReader, DatabaseProviderRW, PruneCheckpointReader, PruneCheckpointWriter,
};
use std::ops::RangeInclusive;
use tracing::error;

pub(crate) trait Part {
    const PART: PrunePart;

    fn prune<DB: Database>(
        &self,
        provider: &DatabaseProviderRW<'_, DB>,
        input: PruneInput,
    ) -> Result<PruneOutput, PrunerError>;

    fn save_checkpoint<DB: Database>(
        &self,
        provider: &DatabaseProviderRW<'_, DB>,
        checkpoint: PruneCheckpoint,
    ) -> RethResult<()> {
        provider.save_prune_checkpoint(Self::PART, checkpoint)
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct PruneInput {
    pub(crate) prune_mode: PruneMode,
    pub(crate) to_block: BlockNumber,
    pub(crate) delete_limit: usize,
}

impl PruneInput {
    /// Get next inclusive tx number range to prune according to the checkpoint and `to_block` block
    /// number.
    ///
    /// To get the range start:
    /// 1. If checkpoint exists, get next block body and return its first tx number.
    /// 2. If checkpoint doesn't exist, return 0.
    ///
    /// To get the range end: get last tx number for `to_block`.
    pub(crate) fn get_next_tx_num_range_from_checkpoint<DB: Database>(
        &self,
        provider: &DatabaseProviderRW<'_, DB>,
        prune_part: PrunePart,
    ) -> RethResult<Option<RangeInclusive<TxNumber>>> {
        let from_tx_number = provider
            .get_prune_checkpoint(prune_part)?
            // Checkpoint exists, prune from the next transaction after the highest pruned one
            .and_then(|checkpoint| match checkpoint.tx_number {
                Some(tx_number) => Some(tx_number + 1),
                _ => {
                    error!(target: "pruner", %prune_part, ?checkpoint, "Expected transaction number in prune checkpoint, found None");
                    None
                },
            })
            // No checkpoint exists, prune from genesis
            .unwrap_or(0);

        let to_tx_number = match provider.block_body_indices(self.to_block)? {
            Some(body) => body,
            None => return Ok(None),
        }
        .last_tx_num();

        let range = from_tx_number..=to_tx_number;
        if range.is_empty() {
            return Ok(None)
        }

        Ok(Some(range))
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct PruneOutput {
    pub(crate) done: bool,
    pub(crate) pruned: usize,
    pub(crate) checkpoint: Option<PruneCheckpoint>,
}

impl PruneOutput {
    pub(crate) fn done() -> Self {
        Self { done: true, pruned: 0, checkpoint: None }
    }
}
