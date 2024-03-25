use crate::{ExecInput, ExecOutput, Stage, StageError, UnwindInput, UnwindOutput};
use futures_util::TryStreamExt;
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW},
    database::Database,
    models::{StoredBlockBodyIndices, StoredBlockOmmers, StoredBlockWithdrawals},
    tables,
    transaction::DbTxMut,
};
use reth_interfaces::{
    p2p::bodies::{downloader::BodyDownloader, response::BlockResponse},
    provider::ProviderResult,
};
use reth_primitives::{
    stage::{EntitiesCheckpoint, StageCheckpoint, StageId},
    StaticFileSegment,
};
use reth_provider::{providers::StaticFileWriter, DatabaseProviderRW, HeaderProvider, StatsReader};
use std::{
    cmp::Ordering,
    task::{ready, Context, Poll},
};
use tracing::*;

// TODO(onbjerg): Metrics and events (gradual status for e.g. CLI)
/// The body stage downloads block bodies.
///
/// The body stage downloads block bodies for all block headers stored locally in storage.
///
/// # Empty blocks
///
/// Blocks with an ommers hash corresponding to no ommers *and* a transaction root corresponding to
/// no transactions will not have a block body downloaded for them, since it would be meaningless to
/// do so.
///
/// This also means that if there is no body for the block in storage (assuming the
/// block number <= the synced block of this stage), then the block can be considered empty.
///
/// # Tables
///
/// The bodies are processed and data is inserted into these tables:
///
/// - [`BlockOmmers`][reth_db::tables::BlockOmmers]
/// - [`BlockBodies`][reth_db::tables::BlockBodyIndices]
/// - [`Transactions`][reth_db::tables::Transactions]
/// - [`TransactionBlocks`][reth_db::tables::TransactionBlocks]
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
    downloader: D,
    /// Block response buffer.
    buffer: Option<Vec<BlockResponse>>,
}

impl<D: BodyDownloader> BodyStage<D> {
    /// Create new bodies stage from downloader.
    pub fn new(downloader: D) -> Self {
        Self { downloader, buffer: None }
    }
}

impl<DB: Database, D: BodyDownloader> Stage<DB> for BodyStage<D> {
    /// Return the id of the stage
    fn id(&self) -> StageId {
        StageId::Bodies
    }

    fn poll_execute_ready(
        &mut self,
        cx: &mut Context<'_>,
        input: ExecInput,
    ) -> Poll<Result<(), StageError>> {
        if input.target_reached() || self.buffer.is_some() {
            return Poll::Ready(Ok(()))
        }

        // Update the header range on the downloader
        self.downloader.set_download_range(input.next_block_range())?;

        // Poll next downloader item.
        let maybe_next_result = ready!(self.downloader.try_poll_next_unpin(cx));

        // Task downloader can return `None` only if the response relaying channel was closed. This
        // is a fatal error to prevent the pipeline from running forever.
        let response = match maybe_next_result {
            Some(Ok(downloaded)) => {
                self.buffer = Some(downloaded);
                Ok(())
            }
            Some(Err(err)) => Err(err.into()),
            None => Err(StageError::ChannelClosed),
        };
        Poll::Ready(response)
    }

    /// Download block bodies from the last checkpoint for this stage up until the latest synced
    /// header, limited by the stage's batch size.
    fn execute(
        &mut self,
        provider: &DatabaseProviderRW<DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        if input.target_reached() {
            return Ok(ExecOutput::done(input.checkpoint()))
        }
        let (from_block, to_block) = input.next_block_range().into_inner();

        // Cursors used to write bodies, ommers and transactions
        let tx = provider.tx_ref();
        let mut block_indices_cursor = tx.cursor_write::<tables::BlockBodyIndices>()?;
        let mut tx_block_cursor = tx.cursor_write::<tables::TransactionBlocks>()?;
        let mut ommers_cursor = tx.cursor_write::<tables::BlockOmmers>()?;
        let mut withdrawals_cursor = tx.cursor_write::<tables::BlockWithdrawals>()?;

        // Get id for the next tx_num of zero if there are no transactions.
        let mut next_tx_num = tx_block_cursor.last()?.map(|(id, _)| id + 1).unwrap_or_default();

        let static_file_provider = provider.static_file_provider();
        let mut static_file_producer =
            static_file_provider.get_writer(from_block, StaticFileSegment::Transactions)?;

        // Make sure Transactions static file is at the same height. If it's further, this
        // input execution was interrupted previously and we need to unwind the static file.
        let next_static_file_tx_num = static_file_provider
            .get_highest_static_file_tx(StaticFileSegment::Transactions)
            .map(|id| id + 1)
            .unwrap_or_default();

        match next_static_file_tx_num.cmp(&next_tx_num) {
            // If static files are ahead, then we didn't reach the database commit in a previous
            // stage run. So, our only solution is to unwind the static files and proceed from the
            // database expected height.
            Ordering::Greater => static_file_producer
                .prune_transactions(next_static_file_tx_num - next_tx_num, from_block - 1)?,
            // If static files are behind, then there was some corruption or loss of files. This
            // error will trigger an unwind, that will bring the database to the same height as the
            // static files.
            Ordering::Less => {
                let last_block = static_file_provider
                    .get_highest_static_file_block(StaticFileSegment::Transactions)
                    .unwrap_or_default();

                let missing_block =
                    Box::new(provider.sealed_header(last_block + 1)?.unwrap_or_default());

                return Err(StageError::MissingStaticFileData {
                    block: missing_block,
                    segment: StaticFileSegment::Transactions,
                })
            }
            Ordering::Equal => {}
        }

        debug!(target: "sync::stages::bodies", stage_progress = from_block, target = to_block, start_tx_id = next_tx_num, "Commencing sync");

        let buffer = self.buffer.take().ok_or(StageError::MissingDownloadBuffer)?;
        trace!(target: "sync::stages::bodies", bodies_len = buffer.len(), "Writing blocks");
        let mut highest_block = from_block;
        for response in buffer {
            // Write block
            let block_number = response.block_number();

            let block_indices = StoredBlockBodyIndices {
                first_tx_num: next_tx_num,
                tx_count: match &response {
                    BlockResponse::Full(block) => block.body.len() as u64,
                    BlockResponse::Empty(_) => 0,
                },
            };

            // Increment block on static file header.
            if block_number > 0 {
                let appended_block_number = static_file_producer
                    .increment_block(StaticFileSegment::Transactions, block_number)?;

                if appended_block_number != block_number {
                    // This scenario indicates a critical error in the logic of adding new
                    // items. It should be treated as an `expect()` failure.
                    return Err(StageError::InconsistentBlockNumber {
                        segment: StaticFileSegment::Transactions,
                        database: block_number,
                        static_file: appended_block_number,
                    })
                }
            }

            match response {
                BlockResponse::Full(block) => {
                    // write transaction block index
                    if !block.body.is_empty() {
                        tx_block_cursor.append(block_indices.last_tx_num(), block.number)?;
                    }

                    // Write transactions
                    for transaction in block.body {
                        let appended_tx_number = static_file_producer
                            .append_transaction(next_tx_num, transaction.into())?;

                        if appended_tx_number != next_tx_num {
                            // This scenario indicates a critical error in the logic of adding new
                            // items. It should be treated as an `expect()` failure.
                            return Err(StageError::InconsistentTxNumber {
                                segment: StaticFileSegment::Transactions,
                                database: next_tx_num,
                                static_file: appended_tx_number,
                            })
                        }

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
    fn unwind(
        &mut self,
        provider: &DatabaseProviderRW<DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        self.buffer.take();

        let static_file_provider = provider.static_file_provider();
        let tx = provider.tx_ref();
        // Cursors to unwind bodies, ommers
        let mut body_cursor = tx.cursor_write::<tables::BlockBodyIndices>()?;
        let mut ommers_cursor = tx.cursor_write::<tables::BlockOmmers>()?;
        let mut withdrawals_cursor = tx.cursor_write::<tables::BlockWithdrawals>()?;
        // Cursors to unwind transitions
        let mut tx_block_cursor = tx.cursor_write::<tables::TransactionBlocks>()?;

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

            // Delete the current body value
            rev_walker.delete_current()?;
        }

        let mut static_file_producer =
            static_file_provider.latest_writer(StaticFileSegment::Transactions)?;

        // Unwind from static files. Get the current last expected transaction from DB, and match it
        // on static file
        let db_tx_num =
            body_cursor.last()?.map(|(_, block_meta)| block_meta.last_tx_num()).unwrap_or_default();
        let static_file_tx_num: u64 = static_file_provider
            .get_highest_static_file_tx(StaticFileSegment::Transactions)
            .unwrap_or_default();

        // If there are more transactions on database, then we are missing static file data and we
        // need to unwind further.
        if db_tx_num > static_file_tx_num {
            let last_block = static_file_provider
                .get_highest_static_file_block(StaticFileSegment::Transactions)
                .unwrap_or_default();

            let missing_block =
                Box::new(provider.sealed_header(last_block + 1)?.unwrap_or_default());

            return Err(StageError::MissingStaticFileData {
                block: missing_block,
                segment: StaticFileSegment::Transactions,
            })
        }

        // Unwinds static file
        static_file_producer
            .prune_transactions(static_file_tx_num.saturating_sub(db_tx_num), input.unwind_to)?;

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
    provider: &DatabaseProviderRW<DB>,
) -> ProviderResult<EntitiesCheckpoint> {
    Ok(EntitiesCheckpoint {
        processed: provider.count_entries::<tables::BlockBodyIndices>()? as u64,
        // Count only static files entries. If we count the database entries too, we may have
        // duplicates. We're sure that the static files have all entries that database has,
        // because we run the `StaticFileProducer` before starting the pipeline.
        total: provider.static_file_provider().count_entries::<tables::Headers>()? as u64,
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
        runner.db().factory.static_file_provider().commit().unwrap();
        assert_matches!(
            output,
            Ok(ExecOutput { checkpoint: StageCheckpoint {
                block_number,
                stage_checkpoint: Some(StageUnitCheckpoint::Entities(EntitiesCheckpoint {
                    processed, // 1 seeded block body + batch size
                    total // seeded headers
                }))
            }, done: false }) if block_number < 200 &&
                processed == batch_size + 1 && total == previous_stage + 1
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
        runner.db().factory.static_file_provider().commit().unwrap();
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
            }) if processed + 1 == total && total == previous_stage + 1
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
        runner.db().factory.static_file_provider().commit().unwrap();
        assert_matches!(
            first_run,
            Ok(ExecOutput { checkpoint: StageCheckpoint {
                block_number,
                stage_checkpoint: Some(StageUnitCheckpoint::Entities(EntitiesCheckpoint {
                    processed,
                    total
                }))
            }, done: false }) if block_number >= 10 &&
                processed - 1 == batch_size && total == previous_stage + 1
        );
        let first_run_checkpoint = first_run.unwrap().checkpoint;

        // Execute again on top of the previous run
        let input =
            ExecInput { target: Some(previous_stage), checkpoint: Some(first_run_checkpoint) };
        let rx = runner.execute(input);

        // Check that we synced more blocks
        let output = rx.await.unwrap();
        runner.db().factory.static_file_provider().commit().unwrap();
        assert_matches!(
            output,
            Ok(ExecOutput { checkpoint: StageCheckpoint {
                block_number,
                stage_checkpoint: Some(StageUnitCheckpoint::Entities(EntitiesCheckpoint {
                    processed,
                    total
                }))
            }, done: true }) if block_number > first_run_checkpoint.block_number &&
                processed + 1 == total && total == previous_stage + 1
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
        runner.db().factory.static_file_provider().commit().unwrap();
        assert_matches!(
            output,
            Ok(ExecOutput { checkpoint: StageCheckpoint {
                block_number,
                stage_checkpoint: Some(StageUnitCheckpoint::Entities(EntitiesCheckpoint {
                    processed,
                    total
                }))
            }, done: true }) if block_number == previous_stage &&
                processed + 1 == total && total == previous_stage + 1
        );
        let checkpoint = output.unwrap().checkpoint;
        runner
            .validate_db_blocks(input.checkpoint().block_number, checkpoint.block_number)
            .expect("Written block data invalid");

        // Delete a transaction
        let static_file_provider = runner.db().factory.static_file_provider();
        {
            let mut static_file_producer =
                static_file_provider.latest_writer(StaticFileSegment::Transactions).unwrap();
            static_file_producer.prune_transactions(1, checkpoint.block_number).unwrap();
        }
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
            }}) if total == previous_stage + 1
        );

        assert_matches!(runner.validate_unwind(input), Ok(_), "unwind validation");
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
        use futures_util::Stream;
        use reth_db::{
            cursor::DbCursorRO,
            models::{StoredBlockBodyIndices, StoredBlockOmmers},
            static_file::HeaderMask,
            tables,
            test_utils::TempDatabase,
            transaction::{DbTx, DbTxMut},
            DatabaseEnv,
        };
        use reth_interfaces::{
            p2p::{
                bodies::{
                    downloader::{BodyDownloader, BodyDownloaderResult},
                    response::BlockResponse,
                },
                error::DownloadResult,
            },
            test_utils::{
                generators,
                generators::{random_block_range, random_signed_tx},
            },
        };
        use reth_primitives::{
            BlockBody, BlockHash, BlockNumber, Header, SealedBlock, SealedHeader,
            StaticFileSegment, TxNumber, B256,
        };
        use reth_provider::{
            providers::StaticFileWriter, HeaderProvider, ProviderFactory, TransactionsProvider,
        };
        use std::{
            collections::{HashMap, VecDeque},
            ops::RangeInclusive,
            pin::Pin,
            sync::Arc,
            task::{Context, Poll},
        };

        /// The block hash of the genesis block.
        pub(crate) const GENESIS_HASH: B256 = B256::ZERO;

        /// A helper to create a collection of block bodies keyed by their hash.
        pub(crate) fn body_by_hash(block: &SealedBlock) -> (B256, BlockBody) {
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
            responses: HashMap<B256, BlockBody>,
            db: TestStageDB,
            batch_size: u64,
        }

        impl Default for BodyTestRunner {
            fn default() -> Self {
                Self { responses: HashMap::default(), db: TestStageDB::default(), batch_size: 1000 }
            }
        }

        impl BodyTestRunner {
            pub(crate) fn set_batch_size(&mut self, batch_size: u64) {
                self.batch_size = batch_size;
            }

            pub(crate) fn set_responses(&mut self, responses: HashMap<B256, BlockBody>) {
                self.responses = responses;
            }
        }

        impl StageTestRunner for BodyTestRunner {
            type S = BodyStage<TestBodyDownloader>;

            fn db(&self) -> &TestStageDB {
                &self.db
            }

            fn stage(&self) -> Self::S {
                BodyStage::new(TestBodyDownloader::new(
                    self.db.factory.clone(),
                    self.responses.clone(),
                    self.batch_size,
                ))
            }
        }

        impl ExecuteStageTestRunner for BodyTestRunner {
            type Seed = Vec<SealedBlock>;

            fn seed_execution(&mut self, input: ExecInput) -> Result<Self::Seed, TestRunnerError> {
                let start = input.checkpoint().block_number;
                let end = input.target();

                let static_file_provider = self.db.factory.static_file_provider();

                let mut rng = generators::rng();

                // Static files do not support gaps in headers, so we need to generate 0 to end
                let blocks = random_block_range(&mut rng, 0..=end, GENESIS_HASH, 0..2);
                self.db.insert_headers_with_td(blocks.iter().map(|block| &block.header))?;
                if let Some(progress) = blocks.get(start as usize) {
                    // Insert last progress data
                    {
                        let tx = self.db.factory.provider_rw()?.into_tx();
                        let mut static_file_producer = static_file_provider
                            .get_writer(start, StaticFileSegment::Transactions)?;

                        let body = StoredBlockBodyIndices {
                            first_tx_num: 0,
                            tx_count: progress.body.len() as u64,
                        };

                        static_file_producer.set_block_range(0..=progress.number);

                        body.tx_num_range().try_for_each(|tx_num| {
                            let transaction = random_signed_tx(&mut rng);
                            static_file_producer
                                .append_transaction(tx_num, transaction.into())
                                .map(|_| ())
                        })?;

                        if body.tx_count != 0 {
                            tx.put::<tables::TransactionBlocks>(
                                body.last_tx_num(),
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

                        static_file_producer.commit()?;
                        tx.commit()?;
                    }
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
                self.db.ensure_no_entry_above::<tables::BlockBodyIndices, _>(
                    input.unwind_to,
                    |key| key,
                )?;
                self.db
                    .ensure_no_entry_above::<tables::BlockOmmers, _>(input.unwind_to, |key| key)?;
                if let Some(last_tx_id) = self.get_last_tx_id()? {
                    self.db
                        .ensure_no_entry_above::<tables::Transactions, _>(last_tx_id, |key| key)?;
                    self.db.ensure_no_entry_above::<tables::TransactionBlocks, _>(
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
                let last_body = self.db.query(|tx| {
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
                let static_file_provider = self.db.factory.static_file_provider();

                self.db.query(|tx| {
                    // Acquire cursors on body related tables
                    let mut bodies_cursor = tx.cursor_read::<tables::BlockBodyIndices>()?;
                    let mut ommers_cursor = tx.cursor_read::<tables::BlockOmmers>()?;
                    let mut tx_block_cursor = tx.cursor_read::<tables::TransactionBlocks>()?;

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

                        let header = static_file_provider.header_by_number(number)?.expect("to be present");
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
                            assert!(static_file_provider.transaction_by_id(tx_id)?.is_some(), "Transaction is missing.");
                        }

                        prev_number = Some(number);
                    }
                    Ok(())
                })?;
                Ok(())
            }
        }

        /// A [BodyDownloader] that is backed by an internal [HashMap] for testing.
        #[derive(Debug)]
        pub(crate) struct TestBodyDownloader {
            provider_factory: ProviderFactory<Arc<TempDatabase<DatabaseEnv>>>,
            responses: HashMap<B256, BlockBody>,
            headers: VecDeque<SealedHeader>,
            batch_size: u64,
        }

        impl TestBodyDownloader {
            pub(crate) fn new(
                provider_factory: ProviderFactory<Arc<TempDatabase<DatabaseEnv>>>,
                responses: HashMap<B256, BlockBody>,
                batch_size: u64,
            ) -> Self {
                Self { provider_factory, responses, headers: VecDeque::default(), batch_size }
            }
        }

        impl BodyDownloader for TestBodyDownloader {
            fn set_download_range(
                &mut self,
                range: RangeInclusive<BlockNumber>,
            ) -> DownloadResult<()> {
                let static_file_provider = self.provider_factory.static_file_provider();

                for header in static_file_provider.fetch_range_iter(
                    StaticFileSegment::Headers,
                    *range.start()..*range.end() + 1,
                    |cursor, number| cursor.get_two::<HeaderMask<Header, BlockHash>>(number.into()),
                )? {
                    let (header, hash) = header?;
                    self.headers.push_back(header.seal(hash));
                }

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
