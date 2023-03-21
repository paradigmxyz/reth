use crate::{
    exec_or_return, ExecAction, ExecInput, ExecOutput, Stage, StageError, StageId, UnwindInput,
    UnwindOutput,
};
use futures_util::TryStreamExt;
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW},
    database::Database,
    models::{StoredBlockBody, StoredBlockOmmers, StoredBlockWithdrawals},
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_interfaces::{
    consensus::Consensus,
    p2p::bodies::{downloader::BodyDownloader, response::BlockResponse},
    provider::ProviderError,
};
use reth_provider::Transaction;
use std::sync::Arc;
use tracing::*;

/// The [`StageId`] of the bodies downloader stage.
pub const BODIES: StageId = StageId("Bodies");

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
/// - [`BlockBodies`][reth_db::tables::BlockBodies]
/// - [`Transactions`][reth_db::tables::Transactions]
/// - [`TransactionBlock`][reth_db::tables::TransactionBlock]
/// - [`BlockTransitionIndex`][reth_db::tables::BlockTransitionIndex]
/// - [`TxTransitionIndex`][reth_db::tables::TxTransitionIndex]
///
/// # Genesis
///
/// This stage expects that the genesis has been inserted into the appropriate tables:
///
/// - The header tables (see [`HeaderStage`][crate::stages::HeaderStage])
/// - The [`BlockOmmers`][reth_db::tables::BlockOmmers] table
/// - The [`BlockBodies`][reth_db::tables::BlockBodies] table
/// - The [`Transactions`][reth_db::tables::Transactions] table
/// - The [`BlockTransitionIndex`][reth_db::tables::BlockTransitionIndex] table
/// - The [`TxTransitionIndex`][reth_db::tables::TxTransitionIndex] table
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
        BODIES
    }

    /// Download block bodies from the last checkpoint for this stage up until the latest synced
    /// header, limited by the stage's batch size.
    async fn execute(
        &mut self,
        tx: &mut Transaction<'_, DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        let (start_block, end_block) = exec_or_return!(input, "sync::stages::bodies");

        // Update the header range on the downloader
        self.downloader.set_download_range(start_block..end_block + 1)?;

        // Cursor used to get total difficulty
        let mut td_cursor = tx.cursor_read::<tables::HeaderTD>()?;

        // Cursors used to write bodies, ommers and transactions
        let mut body_cursor = tx.cursor_write::<tables::BlockBodies>()?;
        let mut tx_cursor = tx.cursor_write::<tables::Transactions>()?;
        let mut tx_block_cursor = tx.cursor_write::<tables::TransactionBlock>()?;
        let mut ommers_cursor = tx.cursor_write::<tables::BlockOmmers>()?;
        let mut withdrawals_cursor = tx.cursor_write::<tables::BlockWithdrawals>()?;

        // Cursors used to write state transition mapping
        let mut block_transition_cursor = tx.cursor_write::<tables::BlockTransitionIndex>()?;
        let mut tx_transition_cursor = tx.cursor_write::<tables::TxTransitionIndex>()?;

        // Get id for the first transaction and first transition in the block
        let (mut current_tx_id, mut transition_id) = tx.get_next_block_ids(start_block)?;

        let mut highest_block = input.stage_progress.unwrap_or_default();
        debug!(target: "sync::stages::bodies", stage_progress = highest_block, target = end_block, start_tx_id = current_tx_id, transition_id, "Commencing sync");

        // Task downloader can return `None` only if the response relaying channel was closed. This
        // is a fatal error to prevent the pipeline from running forever.
        let downloaded_bodies =
            self.downloader.try_next().await?.ok_or(StageError::ChannelClosed)?;

        trace!(target: "sync::stages::bodies", bodies_len = downloaded_bodies.len(), "Writing blocks");
        for response in downloaded_bodies {
            // Write block
            let block_number = response.block_number();
            let difficulty = response.difficulty();

            let mut has_withdrawals = false;
            match response {
                BlockResponse::Full(block) => {
                    let body = StoredBlockBody {
                        start_tx_id: current_tx_id,
                        tx_count: block.body.len() as u64,
                    };
                    body_cursor.append(block_number, body.clone())?;

                    // write transaction block index
                    if !body.is_empty() {
                        tx_block_cursor.append(body.last_tx_index(), block.number)?;
                    }

                    // Write transactions
                    for transaction in block.body {
                        // Append the transaction
                        tx_cursor.append(current_tx_id, transaction)?;
                        tx_transition_cursor.append(current_tx_id, transition_id)?;
                        // Increment transaction id for each transaction.
                        current_tx_id += 1;
                        // Increment transition id for each transaction.
                        transition_id += 1;
                    }

                    // Write ommers if any
                    if !block.ommers.is_empty() {
                        ommers_cursor.append(
                            block_number,
                            StoredBlockOmmers {
                                ommers: block
                                    .ommers
                                    .into_iter()
                                    .map(|header| header.unseal())
                                    .collect(),
                            },
                        )?;
                    }

                    // Write withdrawals if any
                    if let Some(withdrawals) = block.withdrawals {
                        if !withdrawals.is_empty() {
                            has_withdrawals = true;
                            withdrawals_cursor
                                .append(block_number, StoredBlockWithdrawals { withdrawals })?;
                        }
                    }
                }
                BlockResponse::Empty(_) => {
                    body_cursor.append(
                        block_number,
                        StoredBlockBody { start_tx_id: current_tx_id, tx_count: 0 },
                    )?;
                }
            };

            // The block transition marks the final state at the end of the block.
            // Increment the transition if the block contains an addition block reward.
            // If the block does not have a reward, the transition will be the same as the
            // transition at the last transaction of this block.
            let td = td_cursor
                .seek(block_number)?
                .ok_or(ProviderError::TotalDifficulty { number: block_number })?
                .1;
            let has_reward = self.consensus.has_block_reward(td.into(), difficulty);
            let has_post_block_transition = has_reward || has_withdrawals;
            if has_post_block_transition {
                transition_id += 1;
            }
            block_transition_cursor.append(block_number, transition_id)?;

            highest_block = block_number;
        }

        // The stage is "done" if:
        // - We got fewer blocks than our target
        // - We reached our target and the target was not limited by the batch size of the stage
        let done = highest_block == end_block;
        info!(target: "sync::stages::bodies", stage_progress = highest_block, target = end_block, done, "Sync iteration finished");
        Ok(ExecOutput { stage_progress: highest_block, done })
    }

    /// Unwind the stage.
    async fn unwind(
        &mut self,
        tx: &mut Transaction<'_, DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        info!(target: "sync::stages::bodies", to_block = input.unwind_to, "Unwinding");
        // Cursors to unwind bodies, ommers
        let mut body_cursor = tx.cursor_write::<tables::BlockBodies>()?;
        let mut transaction_cursor = tx.cursor_write::<tables::Transactions>()?;
        let mut ommers_cursor = tx.cursor_write::<tables::BlockOmmers>()?;
        let mut withdrawals_cursor = tx.cursor_write::<tables::BlockWithdrawals>()?;
        // Cursors to unwind transitions
        let mut block_transition_cursor = tx.cursor_write::<tables::BlockTransitionIndex>()?;
        let mut tx_transition_cursor = tx.cursor_write::<tables::TxTransitionIndex>()?;
        let mut tx_block_cursor = tx.cursor_write::<tables::TransactionBlock>()?;

        let mut rev_walker = body_cursor.walk_back(None)?;
        while let Some((number, body)) = rev_walker.next().transpose()? {
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

            // Delete the block transition if any
            if block_transition_cursor.seek_exact(number)?.is_some() {
                block_transition_cursor.delete_current()?;
            }

            // Delete all transaction to block values.
            if !body.is_empty() && tx_block_cursor.seek_exact(body.last_tx_index())?.is_some() {
                tx_block_cursor.delete_current()?;
            }

            // Delete all transactions that belong to this block
            for tx_id in body.tx_id_range() {
                // First delete the transaction
                if transaction_cursor.seek_exact(tx_id)?.is_some() {
                    transaction_cursor.delete_current()?;
                }
                // Delete the transaction transition if any
                if tx_transition_cursor.seek_exact(tx_id)?.is_some() {
                    tx_transition_cursor.delete_current()?;
                }
            }

            // Delete the current body value
            tx.delete::<tables::BlockBodies>(number, None)?;
        }

        Ok(UnwindOutput { stage_progress: input.unwind_to })
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
    use test_utils::*;

    stage_test_suite_ext!(BodyTestRunner, body);

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
                let mut tx_cursor = tx.cursor_write::<tables::Transactions>()?;
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
            mdbx::{Env, WriteMap},
            models::{StoredBlockBody, StoredBlockOmmers},
            tables,
            transaction::{DbTx, DbTxMut},
        };
        use reth_interfaces::{
            consensus::Consensus,
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
                generators::{random_block_range, random_signed_tx},
                TestConsensus,
            },
        };
        use reth_primitives::{BlockBody, BlockNumber, SealedBlock, SealedHeader, TxNumber, H256};
        use std::{
            collections::{HashMap, VecDeque},
            ops::Range,
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
                    ommers: block.ommers.iter().cloned().map(|ommer| ommer.unseal()).collect(),
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
                let start = input.stage_progress.unwrap_or_default();
                let end = input.previous_stage_progress() + 1;
                let blocks = random_block_range(start..end, GENESIS_HASH, 0..2);
                self.tx.insert_headers_with_td(blocks.iter().map(|block| &block.header))?;
                if let Some(progress) = blocks.first() {
                    // Insert last progress data
                    self.tx.commit(|tx| {
                        let body = StoredBlockBody {
                            start_tx_id: 0,
                            tx_count: progress.body.len() as u64,
                        };
                        body.tx_id_range().try_for_each(|tx_id| {
                            let transaction = random_signed_tx();
                            tx.put::<tables::Transactions>(tx_id, transaction)?;
                            tx.put::<tables::TxTransitionIndex>(tx_id, tx_id)
                        })?;

                        let last_transition_id = progress.body.len() as u64;
                        let block_transition_id = last_transition_id + 1; // for block reward

                        tx.put::<tables::BlockTransitionIndex>(
                            progress.number,
                            block_transition_id,
                        )?;
                        if body.tx_count != 0 {
                            tx.put::<tables::TransactionBlock>(
                                body.first_tx_index(),
                                progress.number,
                            )?;
                        }
                        tx.put::<tables::BlockBodies>(progress.number, body)?;
                        if !progress.ommers_hash_is_empty() {
                            tx.put::<tables::BlockOmmers>(
                                progress.number,
                                StoredBlockOmmers {
                                    ommers: progress
                                        .ommers
                                        .iter()
                                        .map(|o| o.clone().unseal())
                                        .collect(),
                                },
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
                    Some(output) => output.stage_progress,
                    None => input.stage_progress.unwrap_or_default(),
                };
                self.validate_db_blocks(highest_block, highest_block)
            }
        }

        impl UnwindStageTestRunner for BodyTestRunner {
            fn validate_unwind(&self, input: UnwindInput) -> Result<(), TestRunnerError> {
                self.tx
                    .ensure_no_entry_above::<tables::BlockBodies, _>(input.unwind_to, |key| key)?;
                self.tx
                    .ensure_no_entry_above::<tables::BlockOmmers, _>(input.unwind_to, |key| key)?;
                self.tx.ensure_no_entry_above::<tables::BlockTransitionIndex, _>(
                    input.unwind_to,
                    |key| key,
                )?;
                if let Some(last_tx_id) = self.get_last_tx_id()? {
                    self.tx
                        .ensure_no_entry_above::<tables::Transactions, _>(last_tx_id, |key| key)?;
                    self.tx.ensure_no_entry_above::<tables::TxTransitionIndex, _>(
                        last_tx_id,
                        |key| key,
                    )?;
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
                    let v = tx.cursor_read::<tables::BlockBodies>()?.last()?;
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
                    let mut headers_cursor = tx.cursor_read::<tables::Headers>()?;
                    let mut td_cursor = tx.cursor_read::<tables::HeaderTD>()?;
                    let mut bodies_cursor = tx.cursor_read::<tables::BlockBodies>()?;
                    let mut ommers_cursor = tx.cursor_read::<tables::BlockOmmers>()?;
                    let mut block_transition_cursor = tx.cursor_read::<tables::BlockTransitionIndex>()?;
                    let mut transaction_cursor = tx.cursor_read::<tables::Transactions>()?;
                    let mut tx_transition_cursor = tx.cursor_read::<tables::TxTransitionIndex>()?;
                    let mut tx_block_cursor = tx.cursor_read::<tables::TransactionBlock>()?;

                    let first_body_key = match bodies_cursor.first()? {
                        Some((key, _)) => key,
                        None => return Ok(()),
                    };

                    let mut prev_number: Option<BlockNumber> = None;
                    let mut expected_transition_id = 0;

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

                        let tx_block_id = tx_block_cursor.seek_exact(body.last_tx_index())?.map(|(_,b)| b);
                        if body.tx_count == 0 {
                            assert_ne!(tx_block_id,Some(number));
                        } else {
                            assert_eq!(tx_block_id, Some(number));
                        }

                        for tx_id in body.tx_id_range() {
                            let tx_entry = transaction_cursor.seek_exact(tx_id)?;
                            assert!(tx_entry.is_some(), "Transaction is missing.");
                            assert_eq!(
                                tx_transition_cursor.seek_exact(tx_id).expect("to be okay").expect("to be present").1, expected_transition_id
                            );
                            // Increment expected id for each transaction transition.
                            expected_transition_id += 1;
                        }

                        // Increment expected id for block reward.
                        let td = td_cursor
                            .seek(number)?
                            .expect("Missing TD for header")
                            .1;
                        if self.consensus.has_block_reward(td.into(), header.difficulty) {
                            expected_transition_id += 1;
                        }

                        // Validate that block transition exists
                        assert_eq!(block_transition_cursor.seek_exact(number).expect("To be okay").expect("Block transition to be present").1,expected_transition_id);


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
            db: Arc<Env<WriteMap>>,
            responses: HashMap<H256, BlockBody>,
            headers: VecDeque<SealedHeader>,
            batch_size: u64,
        }

        impl TestBodyDownloader {
            pub(crate) fn new(
                db: Arc<Env<WriteMap>>,
                responses: HashMap<H256, BlockBody>,
                batch_size: u64,
            ) -> Self {
                Self { db, responses, headers: VecDeque::default(), batch_size }
            }
        }

        impl BodyDownloader for TestBodyDownloader {
            fn set_download_range(&mut self, range: Range<BlockNumber>) -> DownloadResult<()> {
                self.headers =
                    VecDeque::from(self.db.view(|tx| -> DownloadResult<Vec<SealedHeader>> {
                        let mut header_cursor = tx.cursor_read::<tables::Headers>()?;

                        let mut canonical_cursor = tx.cursor_read::<tables::CanonicalHeaders>()?;
                        let walker = canonical_cursor.walk_range(range.start..range.end)?;

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
                            ommers: body.ommers.into_iter().map(|h| h.seal_slow()).collect(),
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
