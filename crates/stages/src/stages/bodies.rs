use crate::{ExecInput, ExecOutput, Stage, StageError, UnwindInput, UnwindOutput};
use futures_util::TryStreamExt;
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW},
    database::Database,
    models::{StoredBlockBodyIndices, StoredBlockOmmers, StoredBlockWithdrawals},
    tables,
    transaction::{DbTx, DbTxMut},
    DatabaseError,
};
use reth_interfaces::{
    consensus::Consensus,
    p2p::bodies::{downloader::BodyDownloader, response::BlockResponse},
};
use reth_primitives::stage::{EntitiesCheckpoint, StageCheckpoint, StageId};
use reth_provider::DatabaseProviderRW;
use std::sync::Arc;
use tracing::*;

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
/// - [`BlockOmmers`][reth_db::tables::BlockOmmers]
/// - [`BlockBodies`][reth_db::tables::BlockBodyIndices]
/// - [`Transactions`][reth_db::tables::Transactions]
/// - [`TransactionBlock`][reth_db::tables::TransactionBlock]
///
/// # Genesis
///
/// This stage expects that the genesis has been inserted into the appropriate tables:
///
/// - The header tables (see [`HeaderStage`][crate::stages::HeaderStage])
/// - The [`BlockOmmers`][reth_db::tables::BlockOmmers] table
/// - The [`BlockBodies`][reth_db::tables::BlockBodyIndices] table
/// - The [`Transactions`][reth_db::tables::Transactions] table
#[derive(Debug)]
pub struct BodyStage<D: BodyDownloader> {
    /// The body downloader.
    pub downloader: D,
    /// The consensus engine.
    pub consensus: Arc<dyn Consensus>,
}

#[async_trait::async_trait]
impl<DB: Database, D: BodyDownloader> Stage<DB> for BodyStage<D> {
    /// Return the id of the stage
    fn id(&self) -> StageId {
        StageId::Bodies
    }

    /// Download block bodies from the last checkpoint for this stage up until the latest synced
    /// header, limited by the stage's batch size.
    async fn execute(
        &mut self,
        provider: &DatabaseProviderRW<'_, &DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        if input.target_reached() {
            return Ok(ExecOutput::done(input.checkpoint()))
        }

        let range = input.next_block_range();
        // Update the header range on the downloader
        self.downloader.set_download_range(range.clone())?;
        let (from_block, to_block) = range.into_inner();

        // Cursors used to write bodies, ommers and transactions
        let tx = provider.tx_ref();
        let mut block_indices_cursor = tx.cursor_write::<tables::BlockBodyIndices>()?;
        let mut tx_cursor = tx.cursor_write::<tables::Transactions>()?;
        let mut tx_block_cursor = tx.cursor_write::<tables::TransactionBlock>()?;
        let mut ommers_cursor = tx.cursor_write::<tables::BlockOmmers>()?;
        let mut withdrawals_cursor = tx.cursor_write::<tables::BlockWithdrawals>()?;

        // Get id for the next tx_num of zero if there are no transactions.
        let mut next_tx_num = tx_cursor.last()?.map(|(id, _)| id + 1).unwrap_or_default();

        debug!(target: "sync::stages::bodies", stage_progress = from_block, target = to_block, start_tx_id = next_tx_num, "Commencing sync");

        // Task downloader can return `None` only if the response relaying channel was closed. This
        // is a fatal error to prevent the pipeline from running forever.
        let downloaded_bodies =
            self.downloader.try_next().await?.ok_or(StageError::ChannelClosed)?;

        trace!(target: "sync::stages::bodies", bodies_len = downloaded_bodies.len(), "Writing blocks");

        let mut highest_block = from_block;
        for response in downloaded_bodies {
            // Write block
            let block_number = response.block_number();

            let block_indices = StoredBlockBodyIndices {
                first_tx_num: next_tx_num,
                tx_count: match &response {
                    BlockResponse::Full(block) => block.body.len() as u64,
                    BlockResponse::Empty(_) => 0,
                },
            };
            match response {
                BlockResponse::Full(block) => {
                    // write transaction block index
                    if !block.body.is_empty() {
                        tx_block_cursor.append(block_indices.last_tx_num(), block.number)?;
                    }

                    // Write transactions
                    for transaction in block.body {
                        // Append the transaction
                        tx_cursor.append(next_tx_num, transaction.into())?;
                        // Increment transaction id for each transaction.
                        next_tx_num += 1;
                    }

                    // Write ommers if any
                    if !block.ommers.is_empty() {
                        ommers_cursor
                            .append(block_number, StoredBlockOmmers { ommers: block.ommers })?;
                    }

                    // Write withdrawals if any
                    if let Some(withdrawals) = block.withdrawals {
                        if !withdrawals.is_empty() {
                            withdrawals_cursor
                                .append(block_number, StoredBlockWithdrawals { withdrawals })?;
                        }
                    }
                }
                BlockResponse::Empty(_) => {}
            };

            // insert block meta
            block_indices_cursor.append(block_number, block_indices)?;

            highest_block = block_number;
        }

        // The stage is "done" if:
        // - We got fewer blocks than our target
        // - We reached our target and the target was not limited by the batch size of the stage
        let done = highest_block == to_block;
        Ok(ExecOutput {
            checkpoint: StageCheckpoint::new(highest_block)
                .with_entities_stage_checkpoint(stage_checkpoint(provider)?),
            done,
        })
    }

    /// Unwind the stage.
    async fn unwind(
        &mut self,
        provider: &DatabaseProviderRW<'_, &DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        let tx = provider.tx_ref();
        // Cursors to unwind bodies, ommers
        let mut body_cursor = tx.cursor_write::<tables::BlockBodyIndices>()?;
        let mut transaction_cursor = tx.cursor_write::<tables::Transactions>()?;
        let mut ommers_cursor = tx.cursor_write::<tables::BlockOmmers>()?;
        let mut withdrawals_cursor = tx.cursor_write::<tables::BlockWithdrawals>()?;
        // Cursors to unwind transitions
        let mut tx_block_cursor = tx.cursor_write::<tables::TransactionBlock>()?;

        let mut rev_walker = body_cursor.walk_back(None)?;
        while let Some((number, block_meta)) = rev_walker.next().transpose()? {
            if number <= input.unwind_to {
                break
            }

            // Delete the ommers entry if any
            if ommers_cursor.seek_exact(number)?.is_some() {
                ommers_cursor.delete_current()?;
            }

            // Delete the withdrawals entry if any
            if withdrawals_cursor.seek_exact(number)?.is_some() {
                withdrawals_cursor.delete_current()?;
            }

            // Delete all transaction to block values.
            if !block_meta.is_empty() &&
                tx_block_cursor.seek_exact(block_meta.last_tx_num())?.is_some()
            {
                tx_block_cursor.delete_current()?;
            }

            // Delete all transactions that belong to this block
            for tx_id in block_meta.tx_num_range() {
                // First delete the transaction
                if transaction_cursor.seek_exact(tx_id)?.is_some() {
                    transaction_cursor.delete_current()?;
                }
            }

            // Delete the current body value
            rev_walker.delete_current()?;
        }

        Ok(UnwindOutput {
            checkpoint: StageCheckpoint::new(input.unwind_to)
                .with_entities_stage_checkpoint(stage_checkpoint(provider)?),
        })
    }
}

// TODO(alexey): ideally, we want to measure Bodies stage progress in bytes, but it's hard to know
//  beforehand how many bytes we need to download. So the good solution would be to measure the
//  progress in gas as a proxy to size. Execution stage uses a similar approach.
fn stage_checkpoint<DB: Database>(
    provider: &DatabaseProviderRW<'_, DB>,
) -> Result<EntitiesCheckpoint, DatabaseError> {
    Ok(EntitiesCheckpoint {
        processed: provider.tx_ref().entries::<tables::BlockBodyIndices>()? as u64,
        total: provider.tx_ref().entries::<tables::Headers>()? as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{
        stage_test_suite_ext, ExecuteStageTestRunner, StageTestRunner, UnwindStageTestRunner,
    };
    use assert_matches::assert_matches;
    use reth_primitives::stage::StageUnitCheckpoint;
    use test_utils::*;

    stage_test_suite_ext!(BodyTestRunner, body);

    /// Checks that the stage downloads at most `batch_size` blocks.
    #[tokio::test]
    async fn partial_body_download() {
        let (stage_progress, previous_stage) = (1, 200);

        // Set up test runner
        let mut runner = BodyTestRunner::default();
        let input = ExecInput {
            target: Some(previous_stage),
            checkpoint: Some(StageCheckpoint::new(stage_progress)),
        };
        runner.seed_execution(input).expect("failed to seed execution");

        // Set the batch size (max we sync per stage execution) to less than the number of blocks
        // the previous stage synced (10 vs 20)
        let batch_size = 10;
        runner.set_batch_size(batch_size);

        // Run the stage
        let rx = runner.execute(input);

        // Check that we only synced around `batch_size` blocks even though the number of blocks
        // synced by the previous stage is higher
        let output = rx.await.unwrap();
        assert_matches!(
            output,
            Ok(ExecOutput { checkpoint: StageCheckpoint {
                block_number,
                stage_checkpoint: Some(StageUnitCheckpoint::Entities(EntitiesCheckpoint {
                    processed, // 1 seeded block body + batch size
                    total // seeded headers
                }))
            }, done: false }) if block_number < 200 &&
                processed == 1 + batch_size && total == previous_stage
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
            target: Some(previous_stage),
            checkpoint: Some(StageCheckpoint::new(stage_progress)),
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
            Ok(ExecOutput {
                checkpoint: StageCheckpoint {
                    block_number: 20,
                    stage_checkpoint: Some(StageUnitCheckpoint::Entities(EntitiesCheckpoint {
                        processed,
                        total
                    }))
                },
                done: true
            }) if processed == total && total == previous_stage
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
            target: Some(previous_stage),
            checkpoint: Some(StageCheckpoint::new(stage_progress)),
        };
        runner.seed_execution(input).expect("failed to seed execution");

        let batch_size = 10;
        runner.set_batch_size(batch_size);

        // Run the stage
        let rx = runner.execute(input);

        // Check that we synced at least 10 blocks
        let first_run = rx.await.unwrap();
        assert_matches!(
            first_run,
            Ok(ExecOutput { checkpoint: StageCheckpoint {
                block_number,
                stage_checkpoint: Some(StageUnitCheckpoint::Entities(EntitiesCheckpoint {
                    processed,
                    total
                }))
            }, done: false }) if block_number >= 10 &&
                processed == 1 + batch_size && total == previous_stage
        );
        let first_run_checkpoint = first_run.unwrap().checkpoint;

        // Execute again on top of the previous run
        let input =
            ExecInput { target: Some(previous_stage), checkpoint: Some(first_run_checkpoint) };
        let rx = runner.execute(input);

        // Check that we synced more blocks
        let output = rx.await.unwrap();
        assert_matches!(
            output,
            Ok(ExecOutput { checkpoint: StageCheckpoint {
                block_number,
                stage_checkpoint: Some(StageUnitCheckpoint::Entities(EntitiesCheckpoint {
                    processed,
                    total
                }))
            }, done: true }) if block_number > first_run_checkpoint.block_number &&
                processed == total && total == previous_stage
        );
        assert_matches!(
            runner.validate_execution(input, output.ok()),
            Ok(_),
            "execution validation"
        );
    }

    /// Checks that the stage unwinds correctly, even if a transaction in a block is missing.
    #[tokio::test]
    async fn unwind_missing_tx() {
        let (stage_progress, previous_stage) = (1, 20);

        // Set up test runner
        let mut runner = BodyTestRunner::default();
        let input = ExecInput {
            target: Some(previous_stage),
            checkpoint: Some(StageCheckpoint::new(stage_progress)),
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
            Ok(ExecOutput { checkpoint: StageCheckpoint {
                block_number,
                stage_checkpoint: Some(StageUnitCheckpoint::Entities(EntitiesCheckpoint {
                    processed,
                    total
                }))
            }, done: true }) if block_number == previous_stage &&
                processed == total && total == previous_stage
        );
        let checkpoint = output.unwrap().checkpoint;
        runner
            .validate_db_blocks(input.checkpoint().block_number, checkpoint.block_number)
            .expect("Written block data invalid");

        // Delete a transaction
        runner
            .tx()
            .commit(|tx| {
                let mut tx_cursor = tx.cursor_write::<tables::Transactions>()?;
                tx_cursor.last()?.expect("Could not read last transaction");
                tx_cursor.delete_current()?;
                Ok(())
            })
            .expect("Could not delete a transaction");

        // Unwind all of it
        let unwind_to = 1;
        let input = UnwindInput { bad_block: None, checkpoint, unwind_to };
        let res = runner.unwind(input).await;
        assert_matches!(
            res,
            Ok(UnwindOutput { checkpoint: StageCheckpoint {
                block_number: 1,
                stage_checkpoint: Some(StageUnitCheckpoint::Entities(EntitiesCheckpoint {
                    processed: 1,
                    total
                }))
            }}) if total == previous_stage
        );

        assert_matches!(runner.validate_unwind(input), Ok(_), "unwind validation");
    }

    mod test_utils {
        use crate::{
            stages::bodies::BodyStage,
            test_utils::{
                ExecuteStageTestRunner, StageTestRunner, TestRunnerError, TestTransaction,
                UnwindStageTestRunner,
            },
            ExecInput, ExecOutput, UnwindInput,
        };
        use futures_util::Stream;
        use reth_db::{
            cursor::DbCursorRO,
            database::Database,
            models::{StoredBlockBodyIndices, StoredBlockOmmers},
            tables,
            transaction::{DbTx, DbTxMut},
            DatabaseEnv,
        };
        use reth_interfaces::{
            p2p::{
                bodies::{
                    client::{BodiesClient, BodiesFut},
                    downloader::{BodyDownloader, BodyDownloaderResult},
                    response::BlockResponse,
                },
                download::DownloadClient,
                error::DownloadResult,
                priority::Priority,
            },
            test_utils::{
                generators,
                generators::{random_block_range, random_signed_tx},
                TestConsensus,
            },
        };
        use reth_primitives::{BlockBody, BlockNumber, SealedBlock, SealedHeader, TxNumber, H256};
        use std::{
            collections::{HashMap, VecDeque},
            ops::RangeInclusive,
            pin::Pin,
            sync::Arc,
            task::{Context, Poll},
        };

        /// The block hash of the genesis block.
        pub(crate) const GENESIS_HASH: H256 = H256::zero();

        /// A helper to create a collection of block bodies keyed by their hash.
        pub(crate) fn body_by_hash(block: &SealedBlock) -> (H256, BlockBody) {
            (
                block.hash(),
                BlockBody {
                    transactions: block.body.clone(),
                    ommers: block.ommers.clone(),
                    withdrawals: block.withdrawals.clone(),
                },
            )
        }

        /// A helper struct for running the [BodyStage].
        pub(crate) struct BodyTestRunner {
            pub(crate) consensus: Arc<TestConsensus>,
            responses: HashMap<H256, BlockBody>,
            tx: TestTransaction,
            batch_size: u64,
        }

        impl Default for BodyTestRunner {
            fn default() -> Self {
                Self {
                    consensus: Arc::new(TestConsensus::default()),
                    responses: HashMap::default(),
                    tx: TestTransaction::default(),
                    batch_size: 1000,
                }
            }
        }

        impl BodyTestRunner {
            pub(crate) fn set_batch_size(&mut self, batch_size: u64) {
                self.batch_size = batch_size;
            }

            pub(crate) fn set_responses(&mut self, responses: HashMap<H256, BlockBody>) {
                self.responses = responses;
            }
        }

        impl StageTestRunner for BodyTestRunner {
            type S = BodyStage<TestBodyDownloader>;

            fn tx(&self) -> &TestTransaction {
                &self.tx
            }

            fn stage(&self) -> Self::S {
                BodyStage {
                    downloader: TestBodyDownloader::new(
                        self.tx.inner_raw(),
                        self.responses.clone(),
                        self.batch_size,
                    ),
                    consensus: self.consensus.clone(),
                }
            }
        }

        #[async_trait::async_trait]
        impl ExecuteStageTestRunner for BodyTestRunner {
            type Seed = Vec<SealedBlock>;

            fn seed_execution(&mut self, input: ExecInput) -> Result<Self::Seed, TestRunnerError> {
                let start = input.checkpoint().block_number;
                let end = input.target();
                let mut rng = generators::rng();
                let blocks = random_block_range(&mut rng, start..=end, GENESIS_HASH, 0..2);
                self.tx.insert_headers_with_td(blocks.iter().map(|block| &block.header))?;
                if let Some(progress) = blocks.first() {
                    // Insert last progress data
                    self.tx.commit(|tx| {
                        let body = StoredBlockBodyIndices {
                            first_tx_num: 0,
                            tx_count: progress.body.len() as u64,
                        };
                        body.tx_num_range().try_for_each(|tx_num| {
                            let transaction = random_signed_tx(&mut rng);
                            tx.put::<tables::Transactions>(tx_num, transaction.into())
                        })?;

                        if body.tx_count != 0 {
                            tx.put::<tables::TransactionBlock>(
                                body.first_tx_num(),
                                progress.number,
                            )?;
                        }

                        tx.put::<tables::BlockBodyIndices>(progress.number, body)?;

                        if !progress.ommers_hash_is_empty() {
                            tx.put::<tables::BlockOmmers>(
                                progress.number,
                                StoredBlockOmmers { ommers: progress.ommers.clone() },
                            )?;
                        }
                        Ok(())
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
                    Some(output) => output.checkpoint,
                    None => input.checkpoint(),
                }
                .block_number;
                self.validate_db_blocks(highest_block, highest_block)
            }
        }

        impl UnwindStageTestRunner for BodyTestRunner {
            fn validate_unwind(&self, input: UnwindInput) -> Result<(), TestRunnerError> {
                self.tx.ensure_no_entry_above::<tables::BlockBodyIndices, _>(
                    input.unwind_to,
                    |key| key,
                )?;
                self.tx
                    .ensure_no_entry_above::<tables::BlockOmmers, _>(input.unwind_to, |key| key)?;
                if let Some(last_tx_id) = self.get_last_tx_id()? {
                    self.tx
                        .ensure_no_entry_above::<tables::Transactions, _>(last_tx_id, |key| key)?;
                    self.tx.ensure_no_entry_above::<tables::TransactionBlock, _>(
                        last_tx_id,
                        |key| key,
                    )?;
                }
                Ok(())
            }
        }

        impl BodyTestRunner {
            /// Get the last available tx id if any
            pub(crate) fn get_last_tx_id(&self) -> Result<Option<TxNumber>, TestRunnerError> {
                let last_body = self.tx.query(|tx| {
                    let v = tx.cursor_read::<tables::BlockBodyIndices>()?.last()?;
                    Ok(v)
                })?;
                Ok(match last_body {
                    Some((_, body)) if body.tx_count != 0 => {
                        Some(body.first_tx_num + body.tx_count - 1)
                    }
                    _ => None,
                })
            }

            /// Validate that the inserted block data is valid
            pub(crate) fn validate_db_blocks(
                &self,
                prev_progress: BlockNumber,
                highest_block: BlockNumber,
            ) -> Result<(), TestRunnerError> {
                self.tx.query(|tx| {
                    // Acquire cursors on body related tables
                    let mut headers_cursor = tx.cursor_read::<tables::Headers>()?;
                    let mut bodies_cursor = tx.cursor_read::<tables::BlockBodyIndices>()?;
                    let mut ommers_cursor = tx.cursor_read::<tables::BlockOmmers>()?;
                    let mut transaction_cursor = tx.cursor_read::<tables::Transactions>()?;
                    let mut tx_block_cursor = tx.cursor_read::<tables::TransactionBlock>()?;

                    let first_body_key = match bodies_cursor.first()? {
                        Some((key, _)) => key,
                        None => return Ok(()),
                    };

                    let mut prev_number: Option<BlockNumber> = None;

                    for entry in bodies_cursor.walk(Some(first_body_key))? {
                        let (number, body) = entry?;

                        // Validate sequentiality only after prev progress,
                        // since the data before is mocked and can contain gaps
                        if number > prev_progress {
                            if let Some(prev_key) = prev_number {
                                assert_eq!(prev_key + 1, number, "Body entries must be sequential");
                            }
                        }

                        // Validate that the current entry is below or equals to the highest allowed block
                        assert!(
                            number <= highest_block,
                            "We wrote a block body outside of our synced range. Found block with number {number}, highest block according to stage is {highest_block}",
                        );

                        let (_, header) = headers_cursor.seek_exact(number)?.expect("to be present");
                        // Validate that ommers exist if any
                        let stored_ommers =  ommers_cursor.seek_exact(number)?;
                        if header.ommers_hash_is_empty() {
                            assert!(stored_ommers.is_none(), "Unexpected ommers entry");
                        } else {
                            assert!(stored_ommers.is_some(), "Missing ommers entry");
                        }

                        let tx_block_id = tx_block_cursor.seek_exact(body.last_tx_num())?.map(|(_,b)| b);
                        if body.tx_count == 0 {
                            assert_ne!(tx_block_id,Some(number));
                        } else {
                            assert_eq!(tx_block_id, Some(number));
                        }

                        for tx_id in body.tx_num_range() {
                            let tx_entry = transaction_cursor.seek_exact(tx_id)?;
                            assert!(tx_entry.is_some(), "Transaction is missing.");
                        }


                        prev_number = Some(number);
                    }
                    Ok(())
                })?;
                Ok(())
            }
        }

        /// A [BodiesClient] that should not be called.
        #[derive(Debug)]
        pub(crate) struct NoopClient;

        impl DownloadClient for NoopClient {
            fn report_bad_message(&self, _: reth_primitives::PeerId) {
                panic!("Noop client should not be called")
            }

            fn num_connected_peers(&self) -> usize {
                panic!("Noop client should not be called")
            }
        }

        impl BodiesClient for NoopClient {
            type Output = BodiesFut;

            fn get_block_bodies_with_priority(
                &self,
                _hashes: Vec<H256>,
                _priority: Priority,
            ) -> Self::Output {
                panic!("Noop client should not be called")
            }
        }

        /// A [BodyDownloader] that is backed by an internal [HashMap] for testing.
        #[derive(Debug)]
        pub(crate) struct TestBodyDownloader {
            db: Arc<DatabaseEnv>,
            responses: HashMap<H256, BlockBody>,
            headers: VecDeque<SealedHeader>,
            batch_size: u64,
        }

        impl TestBodyDownloader {
            pub(crate) fn new(
                db: Arc<DatabaseEnv>,
                responses: HashMap<H256, BlockBody>,
                batch_size: u64,
            ) -> Self {
                Self { db, responses, headers: VecDeque::default(), batch_size }
            }
        }

        impl BodyDownloader for TestBodyDownloader {
            fn set_download_range(
                &mut self,
                range: RangeInclusive<BlockNumber>,
            ) -> DownloadResult<()> {
                self.headers =
                    VecDeque::from(self.db.view(|tx| -> DownloadResult<Vec<SealedHeader>> {
                        let mut header_cursor = tx.cursor_read::<tables::Headers>()?;

                        let mut canonical_cursor = tx.cursor_read::<tables::CanonicalHeaders>()?;
                        let walker = canonical_cursor.walk_range(range)?;

                        let mut headers = Vec::default();
                        for entry in walker {
                            let (num, hash) = entry?;
                            let (_, header) =
                                header_cursor.seek_exact(num)?.expect("missing header");
                            headers.push(header.seal(hash));
                        }
                        Ok(headers)
                    })??);
                Ok(())
            }
        }

        impl Stream for TestBodyDownloader {
            type Item = BodyDownloaderResult;
            fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
                let this = self.get_mut();

                if this.headers.is_empty() {
                    return Poll::Ready(None)
                }

                let mut response = Vec::default();
                while let Some(header) = this.headers.pop_front() {
                    if header.is_empty() {
                        response.push(BlockResponse::Empty(header))
                    } else {
                        let body =
                            this.responses.remove(&header.hash()).expect("requested unknown body");
                        response.push(BlockResponse::Full(SealedBlock {
                            header,
                            body: body.transactions,
                            ommers: body.ommers,
                            withdrawals: body.withdrawals,
                        }));
                    }

                    if response.len() as u64 >= this.batch_size {
                        break
                    }
                }

                if !response.is_empty() {
                    return Poll::Ready(Some(Ok(response)))
                }

                panic!("requested bodies without setting headers")
            }
        }
    }
}
