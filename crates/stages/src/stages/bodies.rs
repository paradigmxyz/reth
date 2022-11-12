use crate::{
    DatabaseIntegrityError, ExecInput, ExecOutput, Stage, StageError, StageId, UnwindInput,
    UnwindOutput,
};
use futures_util::TryStreamExt;
use reth_interfaces::{
    consensus::Consensus,
    db::{
        models::StoredBlockBody, tables, DBContainer, Database, DbCursorRO, DbCursorRW, DbTx,
        DbTxMut,
    },
    p2p::bodies::downloader::Downloader,
};
use reth_primitives::{BlockLocked, SealedHeader};
use std::fmt::Debug;
use tracing::warn;

const BODIES: StageId = StageId("Bodies");

// TODO(onbjerg): Metrics and events (gradual status for e.g. CLI)
/// The body stage downloads block bodies.
/// The body downloader stage.
///
/// The body stage downloads block bodies for all block headers stored locally in the database.
///
/// The bodies are processed and data is inserted into these tables:
///
/// - [`BlockBodies`][reth_interfaces::db::tables::BlockBodies]
/// - [`Transactions`][reth_interfaces::db::tables::Transactions]
#[derive(Debug)]
pub struct BodyStage<D: Downloader, C: Consensus> {
    /// The body downloader.
    pub downloader: D,
    /// The consensus engine.
    pub consensus: C,
    /// The maximum amount of block bodies to process in one stage execution.
    ///
    /// Smaller batch sizes result in less memory usage, but more disk I/O. Larger batch sizes
    /// result in more memory usage, less disk I/O, and more infrequent checkpoints.
    pub batch_size: u64,
}

#[async_trait::async_trait]
impl<DB: Database, D: Downloader, C: Consensus> Stage<DB> for BodyStage<D, C> {
    /// Return the id of the stage
    fn id(&self) -> StageId {
        BODIES
    }

    /// Download block bodies from the last checkpoint for this stage up until the latest synced
    /// header, limited by the stage's batch size.
    async fn execute(
        &mut self,
        db: &mut DBContainer<'_, DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        let tx = db.get_mut();
        let starting_block = input.stage_progress.unwrap_or_default();
        let previous_stage_progress =
            input.previous_stage.as_ref().map(|(_, block)| *block).unwrap_or_default();
        if previous_stage_progress == 0 {
            warn!("The body stage seems to be running first, no work can be completed.");
        }

        let target = previous_stage_progress.min(previous_stage_progress + self.batch_size);

        // Short circuit in case we already reached the target block
        if target <= starting_block {
            return Ok(ExecOutput { stage_progress: target, reached_tip: true, done: true })
        }

        // Cursors used to write bodies and transactions
        let mut bodies_cursor = tx.cursor_mut::<tables::BlockBodies>()?;
        let mut tx_cursor = tx.cursor_mut::<tables::Transactions>()?;
        let mut base_tx_id =
            bodies_cursor.last()?.map(|(_, body)| body.base_tx_id + body.tx_amount).unwrap();

        // Cursor used to look up hashes for blocks we are going to download
        let mut header_hashes_cursor = tx.cursor::<tables::CanonicalHeaders>()?;

        // Cursor used to look up headers for block pre-validation
        let mut header_cursor = tx.cursor::<tables::Headers>()?;

        let mut block_number = starting_block;
        while let Some((header_hash, body)) = self
            .downloader
            .stream_bodies(
                header_hashes_cursor
                    .walk(starting_block)?
                    .filter_map(|item| item.ok().map(|(_, hash)| hash))
                    .take((target - starting_block) as usize),
            )
            .try_next()
            .await
            .map_err(|err| StageError::Internal(err.into()))?
        {
            // Fetch the block header for pre-validation
            let block = BlockLocked {
                header: SealedHeader::new(
                    header_cursor
                        .seek_exact((block_number, header_hash).into())?
                        .ok_or(DatabaseIntegrityError::Header {
                            number: block_number,
                            hash: header_hash,
                        })?
                        .1,
                    header_hash,
                ),
                body: body.transactions,
                // TODO: We should have a type w/o receipts probably, no reason to allocate here
                receipts: vec![],
                ommers: body.ommers.into_iter().map(|header| header.seal()).collect(),
            };

            // Pre-validate the block and unwind if it is invalid
            if let Err(error) = self.consensus.pre_validate_block(&block) {
                return Err(StageError::Validation { block: block_number, error })
            }

            // Write block
            bodies_cursor.append(
                (block_number, header_hash).into(),
                StoredBlockBody {
                    base_tx_id,
                    tx_amount: block.body.len() as u64,
                    ommers: block.ommers.into_iter().map(|header| header.unseal()).collect(),
                },
            )?;

            // Write transactions
            for transaction in block.body {
                tx_cursor.append(base_tx_id, transaction)?;
                base_tx_id += 1;
            }

            block_number += 1;
        }

        // The stage is "done" if:
        // - We got fewer blocks than our target
        // - We reached our target and the target was not limited by the batch size of the stage
        let capped = target < previous_stage_progress;
        let done = block_number < target || !capped;

        Ok(ExecOutput { stage_progress: block_number, reached_tip: true, done })
    }

    /// Unwind the stage.
    async fn unwind(
        &mut self,
        db: &mut DBContainer<'_, DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, Box<dyn std::error::Error + Send + Sync>> {
        let tx = db.get_mut();
        let mut block_body_cursor = tx.cursor_mut::<tables::BlockBodies>()?;
        let mut transaction_cursor = tx.cursor_mut::<tables::Transactions>()?;

        let mut entry = block_body_cursor.last()?;
        while let Some((key, body)) = entry {
            if key.number() <= input.unwind_to {
                break
            }

            for num in 0..body.tx_amount {
                let tx_id = body.base_tx_id + num;
                if transaction_cursor.seek_exact(tx_id)?.is_some() {
                    transaction_cursor.delete_current()?;
                }
            }

            block_body_cursor.delete_current()?;
            entry = block_body_cursor.prev()?;
        }

        Ok(UnwindOutput { stage_progress: input.unwind_to })
    }
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    #[ignore]
    async fn empty_db() {}

    #[tokio::test]
    #[ignore]
    async fn already_reached_target() {}

    #[tokio::test]
    #[ignore]
    async fn partial_body_download() {}

    #[tokio::test]
    #[ignore]
    async fn full_body_download() {}

    #[tokio::test]
    #[ignore]
    async fn pre_validation_failure() {}

    #[tokio::test]
    #[ignore]
    async fn block_tx_pointer_validity() {}

    #[tokio::test]
    #[ignore]
    async fn unwind() {}

    #[tokio::test]
    #[ignore]
    async fn unwind_missing_tx() {}

    mod test_utils {}
}
