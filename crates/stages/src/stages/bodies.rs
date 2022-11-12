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
    p2p::bodies::downloader::BodyDownloader,
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
pub struct BodyStage<D: BodyDownloader, C: Consensus> {
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
impl<DB: Database, D: BodyDownloader, C: Consensus> Stage<DB> for BodyStage<D, C> {
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
            .bodies_stream(
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
            self.consensus
                .pre_validate_block(&block)
                .map_err(|err| StageError::Validation { block: block_number, error: err })?;

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
    use super::*;
    use crate::util::test_utils::StageTestRunner;
    use assert_matches::assert_matches;
    use test_utils::*;

    /// Check that the execution is short-circuited if the database is empty.
    #[tokio::test]
    async fn empty_db() {
        let runner = BodyTestRunner::new(|| concurrent_downloader(uncallable_client()));
        let rx = runner.execute(ExecInput::default());
        assert_matches!(
            rx.await.unwrap(),
            Ok(ExecOutput { stage_progress: 0, reached_tip: true, done: true })
        )
    }

    /// Check that the execution is short-circuited if the target was already reached.
    #[tokio::test]
    async fn already_reached_target() {
        let runner = BodyTestRunner::new(|| concurrent_downloader(uncallable_client()));
        let rx = runner.execute(ExecInput {
            previous_stage: Some((StageId("Headers"), 100)),
            stage_progress: Some(100),
        });
        assert_matches!(
            rx.await.unwrap(),
            Ok(ExecOutput { stage_progress: 100, reached_tip: true, done: true })
        )
    }

    /// Checks that the stage downloads at most `batch_size` blocks.
    #[tokio::test]
    #[ignore]
    async fn partial_body_download() {}

    /// Same as [partial_body_download] except the `batch_size` is not hit.
    #[tokio::test]
    #[ignore]
    async fn full_body_download() {}

    /// Checks that the stage asks to unwind if pre-validation of the block fails.
    #[tokio::test]
    #[ignore]
    async fn pre_validation_failure() {}

    /// Checks that the transaction pointers in the stored block bodies are all valid.
    #[tokio::test]
    #[ignore]
    async fn block_tx_pointer_validity() {}

    /// Checks that the stage unwinds correctly.
    #[tokio::test]
    #[ignore]
    async fn unwind() {}

    /// Checks that the stage unwinds correctly, even if a transaction in a block is missing.
    #[tokio::test]
    #[ignore]
    async fn unwind_missing_tx() {}

    /// Checks that the stage exits if the downloader times out
    /// TODO: We should probably just exit as "OK", commit the blocks we downloaded successfully and
    /// try again?
    #[tokio::test]
    #[ignore]
    async fn downloader_timeout() {}

    /// Checks that the stage exits if a header is missing in the block range we were told to sync
    #[tokio::test]
    #[ignore]
    async fn missing_header() {}

    mod test_utils {
        use crate::{
            stages::bodies::BodyStage,
            util::test_utils::{StageTestDB, StageTestRunner},
        };
        use reth_bodies_downloaders::concurrent::{
            ConcurrentDownloader, ConcurrentDownloaderBuilder,
        };
        use reth_eth_wire::BlockBody;
        use reth_interfaces::{
            p2p::bodies::{
                client::BodiesClient, downloader::BodyDownloader, error::BodiesClientError,
            },
            test_utils::{TestBodiesClient, TestConsensus},
        };
        use reth_primitives::H256;
        use std::sync::Arc;

        /// Builds a [ConcurrentDownloader] with the given [BodiesClient].
        pub(crate) fn concurrent_downloader<C>(client: C) -> ConcurrentDownloader<C>
        where
            C: BodiesClient,
        {
            ConcurrentDownloaderBuilder::default().build(Arc::new(client))
        }

        /// A [BodiesClient] that panics if called.
        pub(crate) fn uncallable_client(
        ) -> TestBodiesClient<fn(H256) -> Result<BlockBody, BodiesClientError>> {
            TestBodiesClient {
                responder: |_| panic!("Block body client was called, but it should not be."),
            }
        }

        /// A helper struct for running the [BodyStage].
        pub(crate) struct BodyTestRunner<F, D>
        where
            D: BodyDownloader,
            F: Fn() -> D,
        {
            downloader_builder: F,
            db: StageTestDB,
        }

        impl<F, D> BodyTestRunner<F, D>
        where
            D: BodyDownloader + 'static,
            F: Fn() -> D + 'static,
        {
            /// Build a new test runner.
            pub(crate) fn new(downloader_builder: F) -> Self {
                BodyTestRunner { downloader_builder, db: StageTestDB::default() }
            }
        }

        impl<F, D> StageTestRunner for BodyTestRunner<F, D>
        where
            D: BodyDownloader + 'static,
            F: Fn() -> D + 'static,
        {
            type S = BodyStage<D, TestConsensus>;

            fn db(&self) -> &StageTestDB {
                &self.db
            }

            fn stage(&self) -> Self::S {
                BodyStage {
                    downloader: (self.downloader_builder)(),
                    consensus: TestConsensus::default(),
                    batch_size: 10,
                }
            }
        }
    }
}
