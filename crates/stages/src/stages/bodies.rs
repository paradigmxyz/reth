use crate::{
    db::Transaction, DatabaseIntegrityError, ExecInput, ExecOutput, Stage, StageError, StageId,
    UnwindInput, UnwindOutput,
};
use futures_util::StreamExt;
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW},
    database::{Database, DatabaseGAT},
    models::{StoredBlockBody, StoredBlockOmmers},
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_interfaces::{
    consensus::Consensus,
    p2p::bodies::downloader::{BlockResponse, BodyDownloader},
};
use reth_primitives::{BlockNumber, SealedHeader};
use std::{fmt::Debug, sync::Arc};
use tracing::{error, warn};

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
/// - [`TransactionHashNumber`][reth_interfaces::db::tables::TransactionHashNumber]
///
/// # Genesis
///
/// This stage expects that the genesis has been inserted into the appropriate tables:
///
/// - The header tables (see [`HeaderStage`][crate::stages::headers::HeaderStage])
/// - The [`BlockOmmers`][reth_interfaces::db::tables::BlockOmmers] table
/// - The [`CumulativeTxCount`][reth_interfaces::db::tables::CumulativeTxCount] table
/// - The [`Transactions`][reth_interfaces::db::tables::Transactions] table
/// - The [`TransactionHashNumber`][reth_interfaces::db::tables::TransactionHashNumber] table
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
    pub commit_threshold: u64,
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
        tx: &mut Transaction<'_, DB>,
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
        let stage_progress = input.stage_progress.unwrap_or_default();
        let starting_block = stage_progress + 1;

        // Short circuit in case we already reached the target block
        let target = previous_stage_progress.min(starting_block + self.commit_threshold);
        if target <= stage_progress {
            return Ok(ExecOutput { stage_progress, done: true })
        }

        let bodies_to_download = self.bodies_to_download::<DB>(tx, starting_block, target)?;

        // Cursors used to write bodies, ommers and transactions
        let mut body_cursor = tx.cursor_mut::<tables::BlockBodies>()?;
        let mut ommers_cursor = tx.cursor_mut::<tables::BlockOmmers>()?;
        let mut tx_cursor = tx.cursor_mut::<tables::Transactions>()?;

        // Cursors used to write state transition mapping
        let mut block_transition_cursor = tx.cursor_mut::<tables::BlockTransitionIndex>()?;
        let mut tx_transition_cursor = tx.cursor_mut::<tables::TxTransitionIndex>()?;

        // Get id for the first transaction in the block
        let (mut current_tx_id, mut transition_id) = tx.get_next_block_ids(starting_block)?;

        // NOTE(onbjerg): The stream needs to live here otherwise it will just create a new iterator
        // on every iteration of the while loop -_-
        let mut bodies_stream = self.downloader.bodies_stream(bodies_to_download.iter());
        let mut highest_block = stage_progress;
        while let Some(result) = bodies_stream.next().await {
            let Ok(response) = result else {
                error!(
                    "Encountered an error downloading block {}: {:?}",
                    highest_block + 1,
                    result.unwrap_err()
                );
                return Ok(ExecOutput {
                    stage_progress: highest_block,
                    done: false,
                })
            };

            // Write block
            let block_header = response.header();
            let block_number = block_header.number;
            let block_key = block_header.num_hash().into();

            match response {
                BlockResponse::Full(block) => {
                    body_cursor.append(
                        block_key,
                        StoredBlockBody {
                            start_tx_id: current_tx_id,
                            tx_count: block.body.len() as u64,
                        },
                    )?;
                    ommers_cursor.append(
                        block_key,
                        StoredBlockOmmers {
                            ommers: block
                                .ommers
                                .into_iter()
                                .map(|header| header.unseal())
                                .collect(),
                        },
                    )?;

                    // Write transactions
                    for transaction in block.body {
                        // Insert the transaction hash to number mapping
                        tx.put::<tables::TxHashNumber>(transaction.hash(), current_tx_id)?;
                        // Append the transaction
                        tx_cursor.append(current_tx_id, transaction)?;
                        tx_transition_cursor.append(current_tx_id, transition_id)?;
                        current_tx_id += 1;
                        transition_id += 1;
                    }
                }
                BlockResponse::Empty(_) => {
                    body_cursor.append(
                        block_key,
                        StoredBlockBody { start_tx_id: current_tx_id, tx_count: 0 },
                    )?;
                }
            };

            // The block transition marks the final state at the end of the block.
            // Increment the transition if the block contains an addition block reward.
            // If the block does not have a reward, the transition will be the same as the
            // transition at the last transaction of this block.
            if self.consensus.has_block_reward(block_number) {
                transition_id += 1;
            }
            block_transition_cursor.append(block_key, transition_id)?;

            highest_block = block_number;
        }

        // The stage is "done" if:
        // - We got fewer blocks than our target
        // - We reached our target and the target was not limited by the batch size of the stage
        let capped = target < previous_stage_progress;
        let done = highest_block < target || !capped;

        Ok(ExecOutput { stage_progress: highest_block, done })
    }

    /// Unwind the stage.
    async fn unwind(
        &mut self,
        tx: &mut Transaction<'_, DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, Box<dyn std::error::Error + Send + Sync>> {
        // Cursors to unwind bodies, ommers, transactions and tx hash to number
        let mut body_cursor = tx.cursor_mut::<tables::BlockBodies>()?;
        let mut ommers_cursor = tx.cursor_mut::<tables::BlockOmmers>()?;
        let mut transaction_cursor = tx.cursor_mut::<tables::Transactions>()?;
        let mut tx_hash_number_cursor = tx.cursor_mut::<tables::TxHashNumber>()?;
        // Cursors to unwind transitions
        let mut block_transition_cursor = tx.cursor_mut::<tables::BlockTransitionIndex>()?;
        let mut tx_transition_cursor = tx.cursor_mut::<tables::TxTransitionIndex>()?;

        // let mut entry = tx_count_cursor.last()?;
        let mut entry = body_cursor.last()?;
        while let Some((key, body)) = entry {
            if key.number() <= input.unwind_to {
                break
            }

            // Delete the ommers value if any
            if ommers_cursor.seek_exact(key)?.is_some() {
                ommers_cursor.delete_current()?;
            }

            // Delete the block transition if any
            if block_transition_cursor.seek_exact(key)?.is_some() {
                block_transition_cursor.delete_current()?;
            }

            // Delete all transactions that belong to this block
            for tx_id in body.tx_id_range() {
                // First delete the transaction and hash to id mapping
                if let Some((_, transaction)) = transaction_cursor.seek_exact(tx_id)? {
                    transaction_cursor.delete_current()?;
                    if tx_hash_number_cursor.seek_exact(transaction.hash)?.is_some() {
                        tx_hash_number_cursor.delete_current()?;
                    }
                }
                // Delete the transaction transition if any
                if tx_transition_cursor.seek_exact(tx_id)?.is_some() {
                    tx_transition_cursor.delete_current()?;
                }
            }

            // Delete the current body value
            body_cursor.delete_current()?;
            // Move the cursor to the previous value
            entry = body_cursor.prev()?;
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
    ) -> Result<Vec<SealedHeader>, StageError> {
        let mut header_cursor = tx.cursor::<tables::Headers>()?;
        let mut header_hashes_cursor = tx.cursor::<tables::CanonicalHeaders>()?;
        let mut walker = header_hashes_cursor
            .walk(starting_block)?
            .take_while(|item| item.as_ref().map_or(false, |(num, _)| *num <= target));

        let mut bodies_to_download = Vec::new();
        while let Some(Ok((block_number, header_hash))) = walker.next() {
            let (_, header) = header_cursor.seek_exact((block_number, header_hash).into())?.ok_or(
                DatabaseIntegrityError::Header { number: block_number, hash: header_hash },
            )?;
            bodies_to_download.push(SealedHeader::new(header, header_hash));
        }

        Ok(bodies_to_download)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{
        stage_test_suite_ext, ExecuteStageTestRunner, StageTestRunner, UnwindStageTestRunner,
        PREV_STAGE_ID,
    };
    use assert_matches::assert_matches;
    use reth_interfaces::{
        consensus,
        p2p::error::{DownloadError, RequestError},
    };
    use std::collections::HashMap;
    use test_utils::*;

    stage_test_suite_ext!(BodyTestRunner);

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
            Ok(ExecOutput { stage_progress, done: false }) if stage_progress < 200
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
        assert_matches!(output, Ok(ExecOutput { stage_progress: 20, done: true }));
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
            Ok(ExecOutput { stage_progress, done: false }) if stage_progress >= 10
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
            Ok(ExecOutput { stage_progress, done: true }) if stage_progress > first_run_progress
        );
        assert!(runner.validate_execution(input, output.ok()).is_ok(), "execution validation");
    }

    /// Checks that the stage returns to the pipeline on validation failure.
    #[tokio::test]
    async fn pre_validation_failure() {
        let (stage_progress, previous_stage) = (1, 20);

        // Set up test runner
        let mut runner = BodyTestRunner::default();

        let input = ExecInput {
            previous_stage: Some((PREV_STAGE_ID, previous_stage)),
            stage_progress: Some(stage_progress),
        };
        let blocks = runner.seed_execution(input).expect("failed to seed execution");

        // Fail validation
        let responses = blocks
            .iter()
            .map(|b| {
                (
                    b.hash(),
                    Err(DownloadError::BlockValidation {
                        hash: b.hash(),
                        error: consensus::Error::BaseFeeMissing,
                    }),
                )
            })
            .collect::<HashMap<_, _>>();
        runner.set_responses(responses);

        // Run the stage
        let rx = runner.execute(input);

        // Check that the error bubbles up
        assert_matches!(
            rx.await.unwrap(),
            Ok(ExecOutput { stage_progress: out_stage_progress, done: false })
                if out_stage_progress == stage_progress
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
            Ok(ExecOutput { stage_progress, done: true }) if stage_progress == previous_stage
        );
        let stage_progress = output.unwrap().stage_progress;
        runner
            .validate_db_blocks(input.stage_progress.unwrap_or_default(), stage_progress)
            .expect("Written block data invalid");

        // Delete a transaction
        runner
            .tx()
            .commit(|tx| {
                let mut tx_cursor = tx.cursor_mut::<tables::Transactions>()?;
                let (_, transaction) = tx_cursor.last()?.expect("Could not read last transaction");
                tx_cursor.delete_current()?;
                tx.delete::<tables::TxHashNumber>(transaction.hash, None)?;
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
        runner.set_responses(HashMap::from([(
            header.hash(),
            Err(DownloadError::RequestError(RequestError::Timeout)),
        )]));

        // Run the stage
        let rx = runner.execute(input);

        // Check that the error bubbles up
        assert_matches!(
            rx.await.unwrap(),
            Ok(ExecOutput { stage_progress: out_stage_progress, done: false })
                if out_stage_progress == stage_progress
        );
        assert!(runner.validate_execution(input, None).is_ok(), "execution validation");
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
        use assert_matches::assert_matches;
        use reth_db::{
            cursor::DbCursorRO,
            models::{BlockNumHash, StoredBlockBody, StoredBlockOmmers},
            tables,
            transaction::{DbTx, DbTxMut},
        };
        use reth_eth_wire::BlockBody;
        use reth_interfaces::{
            p2p::{
                bodies::{
                    client::BodiesClient,
                    downloader::{BlockResponse, BodyDownloader},
                },
                downloader::{DownloadClient, DownloadStream, Downloader},
                error::{DownloadResult, PeerRequestResult},
            },
            test_utils::{
                generators::{random_block_range, random_signed_tx},
                TestConsensus,
            },
        };
        use reth_primitives::{BlockLocked, BlockNumber, SealedHeader, TxNumber, H256};
        use std::{collections::HashMap, sync::Arc};

        /// The block hash of the genesis block.
        pub(crate) const GENESIS_HASH: H256 = H256::zero();

        /// A helper to create a collection of resulted-wrapped block bodies keyed by their hash.
        pub(crate) fn body_by_hash(block: &BlockLocked) -> (H256, DownloadResult<BlockBody>) {
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
            responses: HashMap<H256, DownloadResult<BlockBody>>,
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

            pub(crate) fn set_responses(
                &mut self,
                responses: HashMap<H256, DownloadResult<BlockBody>>,
            ) {
                self.responses = responses;
            }
        }

        impl StageTestRunner for BodyTestRunner {
            type S = BodyStage<TestBodyDownloader, TestConsensus>;

            fn tx(&self) -> &TestTransaction {
                &self.tx
            }

            fn stage(&self) -> Self::S {
                BodyStage {
                    downloader: Arc::new(TestBodyDownloader::new(self.responses.clone())),
                    consensus: self.consensus.clone(),
                    commit_threshold: self.batch_size,
                }
            }
        }

        #[async_trait::async_trait]
        impl ExecuteStageTestRunner for BodyTestRunner {
            type Seed = Vec<BlockLocked>;

            fn seed_execution(&mut self, input: ExecInput) -> Result<Self::Seed, TestRunnerError> {
                let start = input.stage_progress.unwrap_or_default();
                let end = input.previous_stage_progress() + 1;
                let blocks = random_block_range(start..end, GENESIS_HASH, 0..2);
                self.tx.insert_headers(blocks.iter().map(|block| &block.header))?;
                if let Some(progress) = blocks.first() {
                    // Insert last progress data
                    self.tx.commit(|tx| {
                        let key = (progress.number, progress.hash()).into();
                        let body = StoredBlockBody {
                            start_tx_id: 0,
                            tx_count: progress.body.len() as u64,
                        };
                        body.tx_id_range().try_for_each(|tx_id| {
                            let transaction = random_signed_tx();
                            tx.put::<tables::TxHashNumber>(transaction.hash(), tx_id)?;
                            tx.put::<tables::Transactions>(tx_id, transaction)?;
                            tx.put::<tables::TxTransitionIndex>(tx_id, tx_id)
                        })?;

                        // Randomize rewards
                        let has_reward: bool = rand::random();
                        let last_transition_id = progress.body.len().saturating_sub(1) as u64;
                        let block_transition_id =
                            last_transition_id + if has_reward { 1 } else { 0 };

                        tx.put::<tables::BlockTransitionIndex>(key, block_transition_id)?;
                        tx.put::<tables::BlockBodies>(key, body)?;
                        tx.put::<tables::BlockOmmers>(key, StoredBlockOmmers { ommers: vec![] })?;
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
                    Some(output) => output.stage_progress,
                    None => input.stage_progress.unwrap_or_default(),
                };
                self.validate_db_blocks(highest_block, highest_block)
            }
        }

        impl UnwindStageTestRunner for BodyTestRunner {
            fn validate_unwind(&self, input: UnwindInput) -> Result<(), TestRunnerError> {
                self.tx.check_no_entry_above::<tables::BlockBodies, _>(input.unwind_to, |key| {
                    key.number()
                })?;
                self.tx.check_no_entry_above::<tables::BlockOmmers, _>(input.unwind_to, |key| {
                    key.number()
                })?;
                self.tx.check_no_entry_above::<tables::BlockTransitionIndex, _>(
                    input.unwind_to,
                    |key| key.number(),
                )?;
                if let Some(last_tx_id) = self.get_last_tx_id()? {
                    self.tx
                        .check_no_entry_above::<tables::Transactions, _>(last_tx_id, |key| key)?;
                    self.tx.check_no_entry_above::<tables::TxTransitionIndex, _>(
                        last_tx_id,
                        |key| key,
                    )?;
                    self.tx.check_no_entry_above_by_value::<tables::TxHashNumber, _>(
                        last_tx_id,
                        |value| value,
                    )?;
                }
                Ok(())
            }
        }

        impl BodyTestRunner {
            /// Get the last available tx id if any
            pub(crate) fn get_last_tx_id(&self) -> Result<Option<TxNumber>, TestRunnerError> {
                let last_body = self.tx.query(|tx| {
                    let v = tx.cursor::<tables::BlockBodies>()?.last()?;
                    Ok(v)
                })?;
                Ok(match last_body {
                    Some((_, body)) if body.tx_count != 0 => {
                        Some(body.start_tx_id + body.tx_count - 1)
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
                    let mut bodies_cursor = tx.cursor::<tables::BlockBodies>()?;
                    let mut ommers_cursor = tx.cursor::<tables::BlockOmmers>()?;
                    let mut block_transition_cursor = tx.cursor::<tables::BlockTransitionIndex>()?;
                    let mut transaction_cursor = tx.cursor::<tables::Transactions>()?;
                    let mut tx_hash_num_cursor = tx.cursor::<tables::TxHashNumber>()?;
                    let mut tx_transition_cursor = tx.cursor::<tables::TxTransitionIndex>()?;

                    let first_body_key = match bodies_cursor.first()? {
                        Some((key, _)) => key,
                        None => return Ok(()),
                    };

                    let mut prev_key: Option<BlockNumHash> = None;
                   for entry in bodies_cursor.walk(first_body_key)? {
                        let (key, body) = entry?;

                        // Validate sequentiality only after prev progress,
                        // since the data before is mocked and can contain gaps
                        if key.number() > prev_progress {
                            if let Some(prev_key) = prev_key {
                                assert_eq!(prev_key.number() + 1, key.number(), "Body entries must be sequential");
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

                        // Validate that block transition exists
                        assert_matches!(block_transition_cursor.seek_exact(key), Ok(Some(_)), "Block transition is missing");

                        for tx_id in body.tx_id_range() {
                            let tx_entry = transaction_cursor.seek_exact(tx_id)?;
                            assert!(tx_entry.is_some(), "Transaction is missing.");
                            assert_matches!(
                                tx_transition_cursor.seek_exact(tx_id), Ok(Some(_)), "Transaction transition is missing"
                            );
                            assert_matches!(
                                tx_hash_num_cursor.seek_exact(tx_entry.unwrap().1.hash),
                                Ok(Some(_)),
                                "Transaction hash to index mapping is missing."
                            );
                        }

                        prev_key = Some(key);
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

        impl DownloadClient for NoopClient {
            fn report_bad_message(&self, _: reth_primitives::PeerId) {
                panic!("Noop client should not be called")
            }
        }

        #[async_trait::async_trait]
        impl BodiesClient for NoopClient {
            async fn get_block_bodies(&self, _: Vec<H256>) -> PeerRequestResult<Vec<BlockBody>> {
                panic!("Noop client should not be called")
            }
        }

        // TODO(onbjerg): Move
        /// A [BodyDownloader] that is backed by an internal [HashMap] for testing.
        #[derive(Debug, Default, Clone)]
        pub(crate) struct TestBodyDownloader {
            responses: HashMap<H256, DownloadResult<BlockBody>>,
        }

        impl TestBodyDownloader {
            pub(crate) fn new(responses: HashMap<H256, DownloadResult<BlockBody>>) -> Self {
                Self { responses }
            }
        }

        impl Downloader for TestBodyDownloader {
            type Client = NoopClient;
            type Consensus = TestConsensus;

            fn client(&self) -> &Self::Client {
                unreachable!()
            }

            fn consensus(&self) -> &Self::Consensus {
                unreachable!()
            }
        }

        impl BodyDownloader for TestBodyDownloader {
            fn bodies_stream<'a, 'b, I>(&'a self, hashes: I) -> DownloadStream<'a, BlockResponse>
            where
                I: IntoIterator<Item = &'b SealedHeader>,
                <I as IntoIterator>::IntoIter: Send + 'b,
                'b: 'a,
            {
                Box::pin(futures_util::stream::iter(hashes.into_iter().map(|header| {
                    let result = self
                        .responses
                        .get(&header.hash())
                        .expect("Stage tried downloading a block we do not have.")
                        .clone()?;
                    Ok(BlockResponse::Full(BlockLocked {
                        header: header.clone(),
                        body: result.transactions,
                        ommers: result.ommers.into_iter().map(|header| header.seal()).collect(),
                    }))
                })))
            }
        }
    }
}
