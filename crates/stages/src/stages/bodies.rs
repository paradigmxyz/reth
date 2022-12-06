use crate::{
    db::StageDB, DatabaseIntegrityError, ExecInput, ExecOutput, Stage, StageError, StageId,
    UnwindInput, UnwindOutput,
};
use futures_util::TryStreamExt;
use reth_interfaces::{
    consensus::Consensus,
    db::{
        models::StoredBlockOmmers, tables, Database, DatabaseGAT, DbCursorRO, DbCursorRW, DbTx,
        DbTxMut,
    },
    p2p::bodies::downloader::BodyDownloader,
};
use reth_primitives::{
    proofs::{EMPTY_LIST_HASH, EMPTY_ROOT},
    BlockLocked, BlockNumber, SealedHeader, H256,
};
use std::{fmt::Debug, sync::Arc};
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
/// - [`BlockOmmers`][reth_interfaces::db::tables::BlockOmmers]
/// - [`Transactions`][reth_interfaces::db::tables::Transactions]
///
/// # Genesis
///
/// This stage expects that the genesis has been inserted into the appropriate tables:
///
/// - The header tables (see [`HeaderStage`][crate::stages::headers::HeaderStage])
/// - The [`BlockOmmers`][reth_interfaces::db::tables::BlockOmmers] table
/// - The [`CumulativeTxCount`][reth_interfaces::db::tables::CumulativeTxCount] table
/// - The [`Transactions`][reth_interfaces::db::tables::Transactions] table
#[derive(Debug)]
pub struct BodyStage<D: BodyDownloader, C: Consensus> {
    /// The body downloader.
    pub downloader: Arc<D>,
    /// The consensus engine.
    pub consensus: Arc<C>,
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
        db: &mut StageDB<'_, DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        let previous_stage_progress = input.previous_stage_progress();
        if previous_stage_progress == 0 {
            warn!("The body stage seems to be running first, no work can be completed.");
            return Err(StageError::DatabaseIntegrity(DatabaseIntegrityError::BlockBody {
                number: 0,
            }))
        }

        // The block we ended at last sync, and the one we are starting on now
        let previous_block = input.stage_progress.unwrap_or_default();
        let starting_block = previous_block + 1;

        // Short circuit in case we already reached the target block
        let target = previous_stage_progress.min(starting_block + self.batch_size);
        if target <= previous_block {
            return Ok(ExecOutput { stage_progress: target, reached_tip: true, done: true })
        }

        let bodies_to_download = self.bodies_to_download::<DB>(db, starting_block, target)?;

        // Cursors used to write bodies and transactions
        let mut ommers_cursor = db.cursor_mut::<tables::BlockOmmers>()?;
        let mut tx_cursor = db.cursor_mut::<tables::Transactions>()?;
        let mut tx_count_cursor = db.cursor_mut::<tables::CumulativeTxCount>()?;

        // Get id for the first transaction in the block
        let mut first_tx_id = db.get_first_tx_id(starting_block)?;

        // Cursor used to look up headers for block pre-validation
        let mut header_cursor = db.cursor::<tables::Headers>()?;

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
            let key = (block_number, header_hash).into();
            // Additional +1, increments tx count to allow indexing of ChangeSet that contains block
            // reward. This can't be added to last transaction ChangeSet as it would
            // break if block is empty.
            let this_tx_count = first_tx_id +
                block.body.len() as u64 +
                if self.consensus.has_block_reward(block_number) { 1 } else { 0 };
            tx_count_cursor.append(key, this_tx_count)?;
            ommers_cursor.append(
                key,
                StoredBlockOmmers {
                    ommers: block.ommers.into_iter().map(|header| header.unseal()).collect(),
                },
            )?;

            // Write transactions
            for transaction in block.body {
                tx_cursor.append(first_tx_id, transaction)?;
                first_tx_id += 1;
            }

            highest_block = block_number;
            first_tx_id = this_tx_count;
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
        db: &mut StageDB<'_, DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, Box<dyn std::error::Error + Send + Sync>> {
        let mut tx_count_cursor = db.cursor_mut::<tables::CumulativeTxCount>()?;
        let mut block_ommers_cursor = db.cursor_mut::<tables::BlockOmmers>()?;
        let mut transaction_cursor = db.cursor_mut::<tables::Transactions>()?;

        let mut entry = tx_count_cursor.last()?;
        while let Some((key, count)) = entry {
            if key.number() <= input.unwind_to {
                break
            }

            // First delete the current and find the previous cum tx count value
            tx_count_cursor.delete_current()?;
            entry = tx_count_cursor.prev()?;

            if block_ommers_cursor.seek_exact(key)?.is_some() {
                block_ommers_cursor.delete_current()?;
            }

            let prev_count = entry.map(|(_, v)| v).unwrap_or_default();
            for tx_id in prev_count..count {
                // Block reward introduces gaps in transaction (Last tx number can be the gap)
                // this is why we are checking if tx exist or not.
                // NOTE: more performant way is probably to use `prev`/`next` fn. and reduce
                // count by one if block has block reward.
                if transaction_cursor.seek_exact(tx_id)?.is_some() {
                    transaction_cursor.delete_current()?;
                }
            }
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
    use crate::test_utils::{
        stage_test_suite, ExecuteStageTestRunner, StageTestRunner, UnwindStageTestRunner,
        PREV_STAGE_ID,
    };
    use assert_matches::assert_matches;
    use reth_interfaces::{consensus, p2p::error::RequestError};
    use std::collections::HashMap;
    use test_utils::*;

    stage_test_suite!(BodyTestRunner);

    /// Checks that the stage downloads at most `batch_size` blocks.
    #[tokio::test]
    async fn partial_body_download() {
        let (stage_progress, previous_stage) = (1, 200);

        // Set up test runner
        let mut runner = BodyTestRunner::default();
        let input = ExecInput {
            previous_stage: Some((PREV_STAGE_ID, previous_stage)),
            stage_progress: Some(stage_progress),
        };
        runner.seed_execution(input).expect("failed to seed execution");

        // Set the batch size (max we sync per stage execution) to less than the number of blocks
        // the previous stage synced (10 vs 20)
        runner.set_batch_size(10);

        // Run the stage
        let rx = runner.execute(input);

        // Check that we only synced around `batch_size` blocks even though the number of blocks
        // synced by the previous stage is higher
        let output = rx.await.unwrap();
        assert_matches!(
            output,
            Ok(ExecOutput { stage_progress, reached_tip: true, done: false }) if stage_progress < 200
        );
        assert!(runner.validate_execution(input, output.ok()).is_ok(), "execution validation");
    }

    /// Same as [partial_body_download] except the `batch_size` is not hit.
    #[tokio::test]
    async fn full_body_download() {
        let (stage_progress, previous_stage) = (1, 20);

        // Set up test runner
        let mut runner = BodyTestRunner::default();
        let input = ExecInput {
            previous_stage: Some((PREV_STAGE_ID, previous_stage)),
            stage_progress: Some(stage_progress),
        };
        runner.seed_execution(input).expect("failed to seed execution");

        // Set the batch size to more than what the previous stage synced (40 vs 20)
        runner.set_batch_size(40);

        // Run the stage
        let rx = runner.execute(input);

        // Check that we synced all blocks successfully, even though our `batch_size` allows us to
        // sync more (if there were more headers)
        let output = rx.await.unwrap();
        assert_matches!(
            output,
            Ok(ExecOutput { stage_progress: 20, reached_tip: true, done: true })
        );
        assert!(runner.validate_execution(input, output.ok()).is_ok(), "execution validation");
    }

    /// Same as [full_body_download] except we have made progress before
    #[tokio::test]
    async fn sync_from_previous_progress() {
        let (stage_progress, previous_stage) = (1, 21);

        // Set up test runner
        let mut runner = BodyTestRunner::default();
        let input = ExecInput {
            previous_stage: Some((PREV_STAGE_ID, previous_stage)),
            stage_progress: Some(stage_progress),
        };
        runner.seed_execution(input).expect("failed to seed execution");

        runner.set_batch_size(10);

        // Run the stage
        let rx = runner.execute(input);

        // Check that we synced at least 10 blocks
        let first_run = rx.await.unwrap();
        assert_matches!(
            first_run,
            Ok(ExecOutput { stage_progress, reached_tip: true, done: false }) if stage_progress >= 10
        );
        let first_run_progress = first_run.unwrap().stage_progress;

        // Execute again on top of the previous run
        let input = ExecInput {
            previous_stage: Some((PREV_STAGE_ID, previous_stage)),
            stage_progress: Some(first_run_progress),
        };
        let rx = runner.execute(input);

        // Check that we synced more blocks
        let output = rx.await.unwrap();
        assert_matches!(
            output,
            Ok(ExecOutput { stage_progress, reached_tip: true, done: true }) if stage_progress > first_run_progress
        );
        assert!(runner.validate_execution(input, output.ok()).is_ok(), "execution validation");
    }

    /// Checks that the stage asks to unwind if pre-validation of the block fails.
    #[tokio::test]
    async fn pre_validation_failure() {
        let (stage_progress, previous_stage) = (1, 20);

        // Set up test runner
        let mut runner = BodyTestRunner::default();
        let input = ExecInput {
            previous_stage: Some((PREV_STAGE_ID, previous_stage)),
            stage_progress: Some(stage_progress),
        };
        runner.seed_execution(input).expect("failed to seed execution");

        // Fail validation
        runner.consensus.set_fail_validation(true);

        // Run the stage
        let rx = runner.execute(input);

        // Check that the error bubbles up
        assert_matches!(
            rx.await.unwrap(),
            Err(StageError::Validation { error: consensus::Error::BaseFeeMissing, .. })
        );
        assert!(runner.validate_execution(input, None).is_ok(), "execution validation");
    }

    /// Checks that the stage unwinds correctly, even if a transaction in a block is missing.
    #[tokio::test]
    async fn unwind_missing_tx() {
        let (stage_progress, previous_stage) = (1, 20);

        // Set up test runner
        let mut runner = BodyTestRunner::default();
        let input = ExecInput {
            previous_stage: Some((PREV_STAGE_ID, previous_stage)),
            stage_progress: Some(stage_progress),
        };
        runner.seed_execution(input).expect("failed to seed execution");

        // Set the batch size to more than what the previous stage synced (40 vs 20)
        runner.set_batch_size(40);

        // Run the stage
        let rx = runner.execute(input);

        // Check that we synced all blocks successfully, even though our `batch_size` allows us to
        // sync more (if there were more headers)
        let output = rx.await.unwrap();
        assert_matches!(
            output,
            Ok(ExecOutput { stage_progress, reached_tip: true, done: true }) if stage_progress == previous_stage
        );
        let stage_progress = output.unwrap().stage_progress;
        runner
            .validate_db_blocks(input.stage_progress.unwrap_or_default(), stage_progress)
            .expect("Written block data invalid");

        // Delete a transaction
        runner
            .db()
            .commit(|tx| {
                let mut tx_cursor = tx.cursor_mut::<tables::Transactions>()?;
                tx_cursor.last()?.expect("Could not read last transaction");
                tx_cursor.delete_current()?;
                Ok(())
            })
            .expect("Could not delete a transaction");

        // Unwind all of it
        let unwind_to = 1;
        let input = UnwindInput { bad_block: None, stage_progress, unwind_to };
        let res = runner.unwind(input).await;
        assert_matches!(
            res,
            Ok(UnwindOutput { stage_progress }) if stage_progress == 1
        );

        assert!(runner.validate_unwind(input).is_ok(), "unwind validation");
    }

    /// Checks that the stage exits if the downloader times out
    /// TODO: We should probably just exit as "OK", commit the blocks we downloaded successfully and
    /// try again?
    #[tokio::test]
    async fn downloader_timeout() {
        let (stage_progress, previous_stage) = (1, 2);

        // Set up test runner
        let mut runner = BodyTestRunner::default();
        let input = ExecInput {
            previous_stage: Some((PREV_STAGE_ID, previous_stage)),
            stage_progress: Some(stage_progress),
        };
        let blocks = runner.seed_execution(input).expect("failed to seed execution");

        // overwrite responses
        let header = blocks.last().unwrap();
        runner.set_responses(HashMap::from([(header.hash(), Err(RequestError::Timeout))]));

        // Run the stage
        let rx = runner.execute(input);

        // Check that the error bubbles up
        assert_matches!(rx.await.unwrap(), Err(StageError::Internal(_)));
        assert!(runner.validate_execution(input, None).is_ok(), "execution validation");
    }

    mod test_utils {
        use crate::{
            stages::bodies::BodyStage,
            test_utils::{
                ExecuteStageTestRunner, StageTestRunner, TestRunnerError, TestStageDB,
                UnwindStageTestRunner,
            },
            ExecInput, ExecOutput, UnwindInput,
        };
        use assert_matches::assert_matches;
        use reth_eth_wire::BlockBody;
        use reth_interfaces::{
            db::{
                models::{BlockNumHash, NumTransactions, StoredBlockOmmers},
                tables, DbCursorRO, DbTx, DbTxMut,
            },
            p2p::{
                bodies::{
                    client::BodiesClient,
                    downloader::{BodiesStream, BodyDownloader},
                },
                error::RequestResult,
            },
            test_utils::{
                generators::{random_block_range, random_signed_tx},
                TestConsensus,
            },
        };
        use reth_primitives::{BlockLocked, BlockNumber, Header, SealedHeader, H256};
        use std::{collections::HashMap, sync::Arc};

        /// The block hash of the genesis block.
        pub(crate) const GENESIS_HASH: H256 = H256::zero();

        /// A helper to create a collection of resulted-wrapped block bodies keyed by their hash.
        pub(crate) fn body_by_hash(block: &BlockLocked) -> (H256, RequestResult<BlockBody>) {
            (
                block.hash(),
                Ok(BlockBody {
                    transactions: block.body.clone(),
                    ommers: block.ommers.iter().cloned().map(|ommer| ommer.unseal()).collect(),
                }),
            )
        }

        /// A helper struct for running the [BodyStage].
        pub(crate) struct BodyTestRunner {
            pub(crate) consensus: Arc<TestConsensus>,
            responses: HashMap<H256, RequestResult<BlockBody>>,
            db: TestStageDB,
            batch_size: u64,
        }

        impl Default for BodyTestRunner {
            fn default() -> Self {
                Self {
                    consensus: Arc::new(TestConsensus::default()),
                    responses: HashMap::default(),
                    db: TestStageDB::default(),
                    batch_size: 1000,
                }
            }
        }

        impl BodyTestRunner {
            pub(crate) fn set_batch_size(&mut self, batch_size: u64) {
                self.batch_size = batch_size;
            }

            pub(crate) fn set_responses(
                &mut self,
                responses: HashMap<H256, RequestResult<BlockBody>>,
            ) {
                self.responses = responses;
            }
        }

        impl StageTestRunner for BodyTestRunner {
            type S = BodyStage<TestBodyDownloader, TestConsensus>;

            fn db(&self) -> &TestStageDB {
                &self.db
            }

            fn stage(&self) -> Self::S {
                BodyStage {
                    downloader: Arc::new(TestBodyDownloader::new(self.responses.clone())),
                    consensus: self.consensus.clone(),
                    batch_size: self.batch_size,
                }
            }
        }

        #[async_trait::async_trait]
        impl ExecuteStageTestRunner for BodyTestRunner {
            type Seed = Vec<BlockLocked>;

            fn seed_execution(&mut self, input: ExecInput) -> Result<Self::Seed, TestRunnerError> {
                self.insert_genesis()?;
                let start = input.stage_progress.unwrap_or_default();
                let end = input.previous_stage_progress() + 1;
                let blocks = random_block_range(start..end, GENESIS_HASH);
                self.db.insert_headers(blocks.iter().map(|block| &block.header))?;
                if let Some(progress) = blocks.first() {
                    // Insert last progress data
                    self.db.commit(|tx| {
                        let key = (progress.number, progress.hash()).into();
                        let last_count = tx
                            .cursor::<tables::CumulativeTxCount>()?
                            .last()?
                            .map(|(_, v)| v)
                            .unwrap_or_default();
                        // +1 for block reward,
                        let tx_count = last_count + progress.body.len() as u64 + 1;
                        tx.put::<tables::CumulativeTxCount>(key, tx_count)?;
                        tx.put::<tables::BlockOmmers>(key, StoredBlockOmmers { ommers: vec![] })?;
                        (last_count..tx_count).try_for_each(|idx| {
                            tx.put::<tables::Transactions>(idx, random_signed_tx())
                        })
                    })?;
                }
                self.set_responses(blocks.iter().map(body_by_hash).collect());
                Ok(blocks)
            }

            fn validate_execution(
                &self,
                input: ExecInput,
                output: Option<ExecOutput>,
            ) -> Result<(), TestRunnerError> {
                let highest_block = match output.as_ref() {
                    Some(output) => output.stage_progress,
                    None => input.stage_progress.unwrap_or_default(),
                };
                self.validate_db_blocks(highest_block, highest_block)
            }
        }

        impl UnwindStageTestRunner for BodyTestRunner {
            fn validate_unwind(&self, input: UnwindInput) -> Result<(), TestRunnerError> {
                self.db.check_no_entry_above::<tables::BlockOmmers, _>(input.unwind_to, |key| {
                    key.number()
                })?;
                self.db.check_no_entry_above::<tables::CumulativeTxCount, _>(
                    input.unwind_to,
                    |key| key.number(),
                )?;
                if let Some(last_tx_id) = self.last_count() {
                    self.db
                        .check_no_entry_above::<tables::Transactions, _>(last_tx_id, |key| key)?;
                }
                Ok(())
            }
        }

        impl BodyTestRunner {
            /// Insert the genesis block into the appropriate tables
            ///
            /// The genesis block always has no transactions and no ommers, and it always has the
            /// same hash.
            pub(crate) fn insert_genesis(&self) -> Result<(), TestRunnerError> {
                let header = SealedHeader::new(Header::default(), GENESIS_HASH);
                self.db.insert_headers(std::iter::once(&header))?;
                self.db.commit(|tx| {
                    let key = (0, GENESIS_HASH).into();
                    tx.put::<tables::CumulativeTxCount>(key, 0)?;
                    tx.put::<tables::BlockOmmers>(key, StoredBlockOmmers { ommers: vec![] })
                })?;

                Ok(())
            }

            /// Retrieve the last tx count from the database
            pub(crate) fn last_count(&self) -> Option<NumTransactions> {
                self.db
                    .query(|tx| Ok(tx.cursor::<tables::CumulativeTxCount>()?.last()?.map(|e| e.1)))
                    .ok()
                    .flatten()
            }

            /// Validate that the inserted block data is valid
            pub(crate) fn validate_db_blocks(
                &self,
                prev_progress: BlockNumber,
                highest_block: BlockNumber,
            ) -> Result<(), TestRunnerError> {
                self.db.query(|tx| {
                    // Acquire cursors on body related tables
                    let mut ommers_cursor = tx.cursor::<tables::BlockOmmers>()?;
                    let mut tx_count_cursor = tx.cursor::<tables::CumulativeTxCount>()?;
                    let mut transaction_cursor = tx.cursor::<tables::Transactions>()?;

                    let first_tx_count_key = match tx_count_cursor.first()? {
                        Some((key, _)) => key,
                        None => return Ok(()),
                    };
                    let mut walker = tx_count_cursor.walk(first_tx_count_key)?.peekable();

                    let mut prev_entry: Option<(BlockNumHash, NumTransactions)> = None;
                    while let Some(entry) = walker.next() {
                        let (key, count) = entry?;

                        // Validate sequentiality only after prev progress,
                        // since the data before is mocked and can contain gaps
                        if key.number() > prev_progress {
                            if let Some((prev_key, _)) = prev_entry {
                                assert_eq!(prev_key.number() + 1, key.number(), "Tx count entries must be sequential");
                            }
                        }

                        // Validate that the current entry is below or equals to the highest allowed block
                        assert!(
                            key.number() <= highest_block,
                            "We wrote a block body outside of our synced range. Found block with number {}, highest block according to stage is {}",
                            key.number(), highest_block
                        );

                        // Validate that ommers exist
                        assert_matches!(ommers_cursor.seek_exact(key), Ok(Some(_)), "Block ommers are missing");

                        // Validate that block trasactions exist
                        let first_tx_id = prev_entry.map(|(_, v)| v).unwrap_or_default();
                        // reduce by one for block_reward index
                        let tx_count = if count == 0 { 0} else {count-1};
                        for tx_id in first_tx_id..tx_count {
                            assert_matches!(
                                transaction_cursor.seek_exact(tx_id),
                                Ok(Some(_)),
                                "A transaction is missing."
                            );
                        }

                        prev_entry = Some((key, count));
                    }
                    Ok(())
                })?;
                Ok(())
            }
        }

        // TODO(onbjerg): Move
        /// A [BodiesClient] that should not be called.
        #[derive(Debug)]
        pub(crate) struct NoopClient;

        #[async_trait::async_trait]
        impl BodiesClient for NoopClient {
            async fn get_block_body(&self, _: Vec<H256>) -> RequestResult<Vec<BlockBody>> {
                panic!("Noop client should not be called")
            }
        }

        // TODO(onbjerg): Move
        /// A [BodyDownloader] that is backed by an internal [HashMap] for testing.
        #[derive(Debug, Default, Clone)]
        pub(crate) struct TestBodyDownloader {
            responses: HashMap<H256, RequestResult<BlockBody>>,
        }

        impl TestBodyDownloader {
            pub(crate) fn new(responses: HashMap<H256, RequestResult<BlockBody>>) -> Self {
                Self { responses }
            }
        }

        impl BodyDownloader for TestBodyDownloader {
            type Client = NoopClient;

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
                        let result = self
                            .responses
                            .get(hash)
                            .expect("Stage tried downloading a block we do not have.")
                            .clone()?;
                        Ok((*block_number, *hash, result))
                    },
                )))
            }
        }
    }
}
