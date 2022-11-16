use crate::{
    DatabaseIntegrityError, ExecInput, ExecOutput, Stage, StageError, StageId, UnwindInput,
    UnwindOutput,
};
use futures_util::TryStreamExt;
use reth_interfaces::{
    consensus::Consensus,
    db::{
        models::StoredBlockBody, tables, DBContainer, Database, DatabaseGAT, DbCursorRO,
        DbCursorRW, DbTx, DbTxMut,
    },
    p2p::bodies::downloader::BodyDownloader,
};
use reth_primitives::{
    proofs::{EMPTY_LIST_HASH, EMPTY_ROOT},
    BlockLocked, BlockNumber, SealedHeader, H256,
};
use std::fmt::Debug;
use tracing::warn;

const BODIES: StageId = StageId("Bodies");

// TODO(onbjerg): Metrics and events (gradual status for e.g. CLI)
/// The body stage downloads block bodies.
///
/// The body stage downloads block bodies for all block headers stored locally in the database.
///
/// # Empty blocks
///
/// Blocks with an ommers hash corresponding to no ommers *and* a transaction root corresponding to
/// no transactions will not have a block body downloaded for them, since it would be meaningless to
/// do so.
///
/// This also means that if there is no body for the block in the database (assuming the
/// block number <= the synced block of this stage), then the block can be considered empty.
///
/// # Tables
///
/// The bodies are processed and data is inserted into these tables:
///
/// - [`BlockBodies`][reth_interfaces::db::tables::BlockBodies]
/// - [`Transactions`][reth_interfaces::db::tables::Transactions]
///
/// # Genesis
///
/// This stage expects that the genesis has been inserted into the appropriate tables:
///
/// - The header tables (see [HeadersStage][crate::stages::headers::HeadersStage])
/// - The various indexes (e.g. [TotalTxIndex][crate::stages::tx_index::TxIndex])
/// - The [`BlockBodies`][reth_interfaces::db::tables::BlockBodies] table
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

        let previous_stage_progress =
            input.previous_stage.as_ref().map(|(_, block)| *block).unwrap_or_default();
        if previous_stage_progress == 0 {
            warn!("The body stage seems to be running first, no work can be completed.");
        }

        // The block we ended at last sync, and the one we are starting on now
        let previous_block = input.stage_progress.unwrap_or_default();
        let starting_block = previous_block + 1;

        // Short circuit in case we already reached the target block
        let target = previous_stage_progress.min(starting_block + self.batch_size);
        if target <= previous_block {
            return Ok(ExecOutput { stage_progress: target, reached_tip: true, done: true })
        }

        let bodies_to_download = self.bodies_to_download::<DB>(tx, starting_block, target)?;

        // Cursors used to write bodies and transactions
        let mut bodies_cursor = tx.cursor_mut::<tables::BlockBodies>()?;
        let mut tx_cursor = tx.cursor_mut::<tables::Transactions>()?;
        let mut base_tx_id = bodies_cursor
            .last()?
            .map(|(_, body)| body.base_tx_id + body.tx_amount)
            .ok_or(DatabaseIntegrityError::BlockBody { number: starting_block })?;

        // Cursor used to look up headers for block pre-validation
        let mut header_cursor = tx.cursor::<tables::Headers>()?;

        // NOTE(onbjerg): The stream needs to live here otherwise it will just create a new iterator
        // on every iteration of the while loop -_-
        let mut bodies_stream = self.downloader.bodies_stream(bodies_to_download.iter());
        let mut highest_block = previous_block;
        while let Some((block_number, header_hash, body)) =
            bodies_stream.try_next().await.map_err(|err| StageError::Internal(err.into()))?
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

            highest_block = block_number;
        }

        // The stage is "done" if:
        // - We got fewer blocks than our target
        // - We reached our target and the target was not limited by the batch size of the stage
        let capped = target < previous_stage_progress;
        let done = highest_block < target || !capped;

        Ok(ExecOutput { stage_progress: highest_block, reached_tip: true, done })
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

impl<D: BodyDownloader, C: Consensus> BodyStage<D, C> {
    /// Computes a list of `(block_number, header_hash)` for blocks that we need to download bodies
    /// for.
    ///
    /// This skips empty blocks (i.e. no ommers, no transactions).
    fn bodies_to_download<DB: Database>(
        &self,
        tx: &mut <DB as DatabaseGAT<'_>>::TXMut,
        starting_block: BlockNumber,
        target: BlockNumber,
    ) -> Result<Vec<(BlockNumber, H256)>, StageError> {
        let mut header_cursor = tx.cursor::<tables::Headers>()?;
        let mut header_hashes_cursor = tx.cursor::<tables::CanonicalHeaders>()?;
        let mut walker = header_hashes_cursor
            .walk(starting_block)?
            .take_while(|item| item.as_ref().map_or(false, |(num, _)| *num <= target));

        let mut bodies_to_download = Vec::new();
        while let Some(Ok((block_number, header_hash))) = walker.next() {
            let header = header_cursor
                .seek_exact((block_number, header_hash).into())?
                .ok_or(DatabaseIntegrityError::Header { number: block_number, hash: header_hash })?
                .1;
            if header.ommers_hash == EMPTY_LIST_HASH && header.transactions_root == EMPTY_ROOT {
                continue
            }

            bodies_to_download.push((block_number, header_hash));
        }

        Ok(bodies_to_download)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::test_utils::StageTestRunner;
    use assert_matches::assert_matches;
    use reth_eth_wire::BlockBody;
    use reth_interfaces::{
        consensus,
        p2p::bodies::error::DownloadError,
        test_utils::generators::{random_block, random_block_range},
    };
    use reth_primitives::{BlockNumber, H256};
    use std::collections::HashMap;
    use test_utils::*;

    /// Check that the execution is short-circuited if the database is empty.
    #[tokio::test]
    async fn empty_db() {
        let runner = BodyTestRunner::new(TestBodyDownloader::default);
        let rx = runner.execute(ExecInput::default());
        assert_matches!(
            rx.await.unwrap(),
            Ok(ExecOutput { stage_progress: 0, reached_tip: true, done: true })
        )
    }

    /// Check that the execution is short-circuited if the target was already reached.
    #[tokio::test]
    async fn already_reached_target() {
        let runner = BodyTestRunner::new(TestBodyDownloader::default);
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
    async fn partial_body_download() {
        // Generate blocks
        let blocks = random_block_range(1..200, GENESIS_HASH);
        let bodies: HashMap<H256, Result<BlockBody, DownloadError>> =
            blocks.iter().map(body_by_hash).collect();
        let mut runner = BodyTestRunner::new(|| TestBodyDownloader::new(bodies.clone()));

        // Set the batch size (max we sync per stage execution) to less than the number of blocks
        // the previous stage synced (10 vs 20)
        runner.set_batch_size(10);

        // Insert required state
        runner.insert_genesis().expect("Could not insert genesis block");
        runner
            .insert_headers(blocks.iter().map(|block| &block.header))
            .expect("Could not insert headers");

        // Run the stage
        let rx = runner.execute(ExecInput {
            previous_stage: Some((StageId("Headers"), blocks.len() as BlockNumber)),
            stage_progress: None,
        });

        // Check that we only synced around `batch_size` blocks even though the number of blocks
        // synced by the previous stage is higher
        let output = rx.await.unwrap();
        assert_matches!(
            output,
            Ok(ExecOutput { stage_progress, reached_tip: true, done: false }) if stage_progress < 200
        );
        runner
            .validate_db_blocks(output.unwrap().stage_progress)
            .expect("Written block data invalid");
    }

    /// Same as [partial_body_download] except the `batch_size` is not hit.
    #[tokio::test]
    async fn full_body_download() {
        // Generate blocks #1-20
        let blocks = random_block_range(1..21, GENESIS_HASH);
        let bodies: HashMap<H256, Result<BlockBody, DownloadError>> =
            blocks.iter().map(body_by_hash).collect();
        let mut runner = BodyTestRunner::new(|| TestBodyDownloader::new(bodies.clone()));

        // Set the batch size to more than what the previous stage synced (40 vs 20)
        runner.set_batch_size(40);

        // Insert required state
        runner.insert_genesis().expect("Could not insert genesis block");
        runner
            .insert_headers(blocks.iter().map(|block| &block.header))
            .expect("Could not insert headers");

        // Run the stage
        let rx = runner.execute(ExecInput {
            previous_stage: Some((StageId("Headers"), blocks.len() as BlockNumber)),
            stage_progress: None,
        });

        // Check that we synced all blocks successfully, even though our `batch_size` allows us to
        // sync more (if there were more headers)
        let output = rx.await.unwrap();
        assert_matches!(
            output,
            Ok(ExecOutput { stage_progress: 20, reached_tip: true, done: true })
        );
        runner
            .validate_db_blocks(output.unwrap().stage_progress)
            .expect("Written block data invalid");
    }

    /// Same as [full_body_download] except we have made progress before
    #[tokio::test]
    async fn sync_from_previous_progress() {
        // Generate blocks #1-20
        let blocks = random_block_range(1..21, GENESIS_HASH);
        let bodies: HashMap<H256, Result<BlockBody, DownloadError>> =
            blocks.iter().map(body_by_hash).collect();
        let runner = BodyTestRunner::new(|| TestBodyDownloader::new(bodies.clone()));

        // Insert required state
        runner.insert_genesis().expect("Could not insert genesis block");
        runner
            .insert_headers(blocks.iter().map(|block| &block.header))
            .expect("Could not insert headers");

        // Run the stage
        let rx = runner.execute(ExecInput {
            previous_stage: Some((StageId("Headers"), blocks.len() as BlockNumber)),
            stage_progress: None,
        });

        // Check that we synced at least 10 blocks
        let first_run = rx.await.unwrap();
        assert_matches!(
            first_run,
            Ok(ExecOutput { stage_progress, reached_tip: true, done: false }) if stage_progress >= 10
        );
        let first_run_progress = first_run.unwrap().stage_progress;

        // Execute again on top of the previous run
        let rx = runner.execute(ExecInput {
            previous_stage: Some((StageId("Headers"), blocks.len() as BlockNumber)),
            stage_progress: Some(first_run_progress),
        });

        // Check that we synced more blocks
        let output = rx.await.unwrap();
        assert_matches!(
            output,
            Ok(ExecOutput { stage_progress, reached_tip: true, done: true }) if stage_progress > first_run_progress
        );
        runner
            .validate_db_blocks(output.unwrap().stage_progress)
            .expect("Written block data invalid");
    }

    /// Checks that the stage asks to unwind if pre-validation of the block fails.
    #[tokio::test]
    async fn pre_validation_failure() {
        // Generate blocks #1-19
        let blocks = random_block_range(1..20, GENESIS_HASH);
        let bodies: HashMap<H256, Result<BlockBody, DownloadError>> =
            blocks.iter().map(body_by_hash).collect();
        let mut runner = BodyTestRunner::new(|| TestBodyDownloader::new(bodies.clone()));

        // Fail validation
        runner.set_fail_validation(true);

        // Insert required state
        runner.insert_genesis().expect("Could not insert genesis block");
        runner
            .insert_headers(blocks.iter().map(|block| &block.header))
            .expect("Could not insert headers");

        // Run the stage
        let rx = runner.execute(ExecInput {
            previous_stage: Some((StageId("Headers"), blocks.len() as BlockNumber)),
            stage_progress: None,
        });

        // Check that the error bubbles up
        assert_matches!(
            rx.await.unwrap(),
            Err(StageError::Validation { block: 1, error: consensus::Error::BaseFeeMissing })
        );
    }

    /// Checks that the stage unwinds correctly with no data.
    #[tokio::test]
    async fn unwind_empty_db() {
        let unwind_to = 10;
        let runner = BodyTestRunner::new(TestBodyDownloader::default);
        let rx = runner.unwind(UnwindInput { bad_block: None, stage_progress: 20, unwind_to });
        assert_matches!(
            rx.await.unwrap(),
            Ok(UnwindOutput { stage_progress }) if stage_progress == unwind_to
        )
    }

    /// Checks that the stage unwinds correctly with data.
    #[tokio::test]
    async fn unwind() {
        // Generate blocks #1-20
        let blocks = random_block_range(1..21, GENESIS_HASH);
        let bodies: HashMap<H256, Result<BlockBody, DownloadError>> =
            blocks.iter().map(body_by_hash).collect();
        let mut runner = BodyTestRunner::new(|| TestBodyDownloader::new(bodies.clone()));

        // Set the batch size to more than what the previous stage synced (40 vs 20)
        runner.set_batch_size(40);

        // Insert required state
        runner.insert_genesis().expect("Could not insert genesis block");
        runner
            .insert_headers(blocks.iter().map(|block| &block.header))
            .expect("Could not insert headers");

        // Run the stage
        let rx = runner.execute(ExecInput {
            previous_stage: Some((StageId("Headers"), blocks.len() as BlockNumber)),
            stage_progress: None,
        });

        // Check that we synced all blocks successfully, even though our `batch_size` allows us to
        // sync more (if there were more headers)
        let output = rx.await.unwrap();
        assert_matches!(
            output,
            Ok(ExecOutput { stage_progress: 20, reached_tip: true, done: true })
        );
        let stage_progress = output.unwrap().stage_progress;
        runner.validate_db_blocks(stage_progress).expect("Written block data invalid");

        // Unwind all of it
        let unwind_to = 1;
        let rx = runner.unwind(UnwindInput { bad_block: None, stage_progress, unwind_to });
        assert_matches!(
            rx.await.unwrap(),
            Ok(UnwindOutput { stage_progress }) if stage_progress == 1
        );

        let last_body = runner.last_body().expect("Could not read last body");
        let last_tx_id = last_body.base_tx_id + last_body.tx_amount;
        runner
            .db()
            .check_no_entry_above::<tables::BlockBodies, _>(unwind_to, |key| key.number())
            .expect("Did not unwind block bodies correctly.");
        runner
            .db()
            .check_no_entry_above::<tables::Transactions, _>(last_tx_id, |key| key)
            .expect("Did not unwind transactions correctly.")
    }

    /// Checks that the stage unwinds correctly, even if a transaction in a block is missing.
    #[tokio::test]
    async fn unwind_missing_tx() {
        // Generate blocks #1-20
        let blocks = random_block_range(1..21, GENESIS_HASH);
        let bodies: HashMap<H256, Result<BlockBody, DownloadError>> =
            blocks.iter().map(body_by_hash).collect();
        let mut runner = BodyTestRunner::new(|| TestBodyDownloader::new(bodies.clone()));

        // Set the batch size to more than what the previous stage synced (40 vs 20)
        runner.set_batch_size(40);

        // Insert required state
        runner.insert_genesis().expect("Could not insert genesis block");
        runner
            .insert_headers(blocks.iter().map(|block| &block.header))
            .expect("Could not insert headers");

        // Run the stage
        let rx = runner.execute(ExecInput {
            previous_stage: Some((StageId("Headers"), blocks.len() as BlockNumber)),
            stage_progress: None,
        });

        // Check that we synced all blocks successfully, even though our `batch_size` allows us to
        // sync more (if there were more headers)
        let output = rx.await.unwrap();
        assert_matches!(
            output,
            Ok(ExecOutput { stage_progress: 20, reached_tip: true, done: true })
        );
        let stage_progress = output.unwrap().stage_progress;
        runner.validate_db_blocks(stage_progress).expect("Written block data invalid");

        // Delete a transaction
        {
            let mut db = runner.db().container();
            let mut tx_cursor = db
                .get_mut()
                .cursor_mut::<tables::Transactions>()
                .expect("Could not get transaction cursor");
            tx_cursor
                .last()
                .expect("Could not read database")
                .expect("Could not read last transaction");
            tx_cursor.delete_current().expect("Could not delete last transaction");
            db.commit().expect("Could not commit database");
        }

        // Unwind all of it
        let unwind_to = 1;
        let rx = runner.unwind(UnwindInput { bad_block: None, stage_progress, unwind_to });
        assert_matches!(
            rx.await.unwrap(),
            Ok(UnwindOutput { stage_progress }) if stage_progress == 1
        );

        let last_body = runner.last_body().expect("Could not read last body");
        let last_tx_id = last_body.base_tx_id + last_body.tx_amount;
        runner
            .db()
            .check_no_entry_above::<tables::BlockBodies, _>(unwind_to, |key| key.number())
            .expect("Did not unwind block bodies correctly.");
        runner
            .db()
            .check_no_entry_above::<tables::Transactions, _>(last_tx_id, |key| key)
            .expect("Did not unwind transactions correctly.")
    }

    /// Checks that the stage exits if the downloader times out
    /// TODO: We should probably just exit as "OK", commit the blocks we downloaded successfully and
    /// try again?
    #[tokio::test]
    async fn downloader_timeout() {
        // Generate a header
        let header = random_block(1, Some(GENESIS_HASH)).header;
        let runner = BodyTestRunner::new(|| {
            TestBodyDownloader::new(HashMap::from([(
                header.hash(),
                Err(DownloadError::Timeout { header_hash: header.hash() }),
            )]))
        });

        // Insert required state
        runner.insert_genesis().expect("Could not insert genesis block");
        runner.insert_header(&header).expect("Could not insert header");

        // Run the stage
        let rx = runner.execute(ExecInput {
            previous_stage: Some((StageId("Headers"), 1)),
            stage_progress: None,
        });

        // Check that the error bubbles up
        assert_matches!(rx.await.unwrap(), Err(StageError::Internal(_)));
    }

    mod test_utils {
        use crate::{
            stages::bodies::BodyStage,
            util::test_utils::{StageTestDB, StageTestRunner},
        };
        use assert_matches::assert_matches;
        use async_trait::async_trait;
        use reth_eth_wire::BlockBody;
        use reth_interfaces::{
            db,
            db::{
                models::{BlockNumHash, StoredBlockBody},
                tables, DbCursorRO, DbTx, DbTxMut,
            },
            p2p::bodies::{
                client::BodiesClient,
                downloader::{BodiesStream, BodyDownloader},
                error::{BodiesClientError, DownloadError},
            },
            test_utils::TestConsensus,
        };
        use reth_primitives::{
            BigEndianHash, BlockLocked, BlockNumber, Header, SealedHeader, H256, U256,
        };
        use std::{collections::HashMap, ops::Deref, time::Duration};

        /// The block hash of the genesis block.
        pub(crate) const GENESIS_HASH: H256 = H256::zero();

        /// A helper to create a collection of resulted-wrapped block bodies keyed by their hash.
        pub(crate) fn body_by_hash(
            block: &BlockLocked,
        ) -> (H256, Result<BlockBody, DownloadError>) {
            (
                block.hash(),
                Ok(BlockBody {
                    transactions: block.body.clone(),
                    ommers: block.ommers.iter().cloned().map(|ommer| ommer.unseal()).collect(),
                }),
            )
        }

        /// A helper struct for running the [BodyStage].
        pub(crate) struct BodyTestRunner<F>
        where
            F: Fn() -> TestBodyDownloader,
        {
            downloader_builder: F,
            db: StageTestDB,
            batch_size: u64,
            fail_validation: bool,
        }

        impl<F> BodyTestRunner<F>
        where
            F: Fn() -> TestBodyDownloader,
        {
            /// Build a new test runner.
            pub(crate) fn new(downloader_builder: F) -> Self {
                BodyTestRunner {
                    downloader_builder,
                    db: StageTestDB::default(),
                    batch_size: 10,
                    fail_validation: false,
                }
            }

            pub(crate) fn set_batch_size(&mut self, batch_size: u64) {
                self.batch_size = batch_size;
            }

            pub(crate) fn set_fail_validation(&mut self, fail_validation: bool) {
                self.fail_validation = fail_validation;
            }
        }

        impl<F> StageTestRunner for BodyTestRunner<F>
        where
            F: Fn() -> TestBodyDownloader,
        {
            type S = BodyStage<TestBodyDownloader, TestConsensus>;

            fn db(&self) -> &StageTestDB {
                &self.db
            }

            fn stage(&self) -> Self::S {
                let mut consensus = TestConsensus::default();
                consensus.set_fail_validation(self.fail_validation);

                BodyStage {
                    downloader: (self.downloader_builder)(),
                    consensus,
                    batch_size: self.batch_size,
                }
            }
        }

        impl<F> BodyTestRunner<F>
        where
            F: Fn() -> TestBodyDownloader,
        {
            /// Insert the genesis block into the appropriate tables
            ///
            /// The genesis block always has no transactions and no ommers, and it always has the
            /// same hash.
            pub(crate) fn insert_genesis(&self) -> Result<(), db::Error> {
                self.insert_header(&SealedHeader::new(Header::default(), GENESIS_HASH))?;
                let mut db = self.db.container();
                let tx = db.get_mut();
                tx.put::<tables::BlockBodies>(
                    (0, GENESIS_HASH).into(),
                    StoredBlockBody { base_tx_id: 0, tx_amount: 0, ommers: vec![] },
                )?;
                db.commit()?;

                Ok(())
            }

            /// Insert header into tables
            pub(crate) fn insert_header(&self, header: &SealedHeader) -> Result<(), db::Error> {
                self.insert_headers(std::iter::once(header))
            }

            /// Insert headers into tables
            pub(crate) fn insert_headers<'a, I>(&self, headers: I) -> Result<(), db::Error>
            where
                I: Iterator<Item = &'a SealedHeader>,
            {
                let headers = headers.collect::<Vec<_>>();
                self.db
                    .map_put::<tables::HeaderNumbers, _, _>(&headers, |h| (h.hash(), h.number))?;
                self.db.map_put::<tables::Headers, _, _>(&headers, |h| {
                    (BlockNumHash((h.number, h.hash())), h.deref().clone().unseal())
                })?;
                self.db.map_put::<tables::CanonicalHeaders, _, _>(&headers, |h| {
                    (h.number, h.hash())
                })?;

                self.db.transform_append::<tables::HeaderTD, _, _>(&headers, |prev, h| {
                    let prev_td = U256::from_big_endian(&prev.clone().unwrap_or_default());
                    (
                        BlockNumHash((h.number, h.hash())),
                        H256::from_uint(&(prev_td + h.difficulty)).as_bytes().to_vec(),
                    )
                })?;

                Ok(())
            }

            pub(crate) fn last_body(&self) -> Option<StoredBlockBody> {
                Some(
                    self.db()
                        .container()
                        .get()
                        .cursor::<tables::BlockBodies>()
                        .ok()?
                        .last()
                        .ok()??
                        .1,
                )
            }

            /// Validate that the inserted block data is valid
            pub(crate) fn validate_db_blocks(
                &self,
                highest_block: BlockNumber,
            ) -> Result<(), db::Error> {
                let db = self.db.container();
                let tx = db.get();

                let mut block_body_cursor = tx.cursor::<tables::BlockBodies>()?;
                let mut transaction_cursor = tx.cursor::<tables::Transactions>()?;

                let mut entry = block_body_cursor.first()?;
                let mut prev_max_tx_id = 0;
                while let Some((key, body)) = entry {
                    assert!(
                        key.number() <= highest_block,
                        "We wrote a block body outside of our synced range. Found block with number {}, highest block according to stage is {}",
                        key.number(), highest_block
                    );

                    assert!(prev_max_tx_id == body.base_tx_id, "Transaction IDs are malformed.");
                    for num in 0..body.tx_amount {
                        let tx_id = body.base_tx_id + num;
                        assert_matches!(
                            transaction_cursor.seek_exact(tx_id),
                            Ok(Some(_)),
                            "A transaction is missing."
                        );
                    }
                    prev_max_tx_id = body.base_tx_id + body.tx_amount;
                    entry = block_body_cursor.next()?;
                }

                Ok(())
            }
        }

        // TODO(onbjerg): Move
        /// A [BodiesClient] that should not be called.
        #[derive(Debug)]
        pub(crate) struct NoopClient;

        #[async_trait]
        impl BodiesClient for NoopClient {
            async fn get_block_body(&self, _: H256) -> Result<BlockBody, BodiesClientError> {
                panic!("Noop client should not be called")
            }
        }

        // TODO(onbjerg): Move
        /// A [BodyDownloader] that is backed by an internal [HashMap] for testing.
        #[derive(Debug, Default)]
        pub(crate) struct TestBodyDownloader {
            responses: HashMap<H256, Result<BlockBody, DownloadError>>,
        }

        impl TestBodyDownloader {
            pub(crate) fn new(responses: HashMap<H256, Result<BlockBody, DownloadError>>) -> Self {
                Self { responses }
            }
        }

        impl BodyDownloader for TestBodyDownloader {
            type Client = NoopClient;

            fn timeout(&self) -> Duration {
                unreachable!()
            }

            fn client(&self) -> &Self::Client {
                unreachable!()
            }

            fn bodies_stream<'a, 'b, I>(&'a self, hashes: I) -> BodiesStream<'a>
            where
                I: IntoIterator<Item = &'b (BlockNumber, H256)>,
                <I as IntoIterator>::IntoIter: Send + 'b,
                'b: 'a,
            {
                Box::pin(futures_util::stream::iter(hashes.into_iter().map(
                    |(block_number, hash)| {
                        Ok((
                            *block_number,
                            *hash,
                            self.responses
                                .get(hash)
                                .expect("Stage tried downloading a block we do not have.")
                                .clone()?,
                        ))
                    },
                )))
            }
        }
    }
}
