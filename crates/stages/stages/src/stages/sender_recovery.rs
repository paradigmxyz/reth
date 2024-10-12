use alloy_primitives::{Address, TxNumber};
use reth_config::config::SenderRecoveryConfig;
use reth_consensus::ConsensusError;
use reth_db::{static_file::TransactionMask, tables, RawValue};
use reth_db_api::{
    cursor::DbCursorRW,
    transaction::{DbTx, DbTxMut},
    DbTxUnwindExt,
};
use reth_primitives::{GotExpected, StaticFileSegment, TransactionSignedNoHash};
use reth_provider::{
    BlockReader, DBProvider, HeaderProvider, ProviderError, PruneCheckpointReader,
    StaticFileProviderFactory, StatsReader,
};
use reth_prune_types::PruneSegment;
use reth_stages_api::{
    BlockErrorKind, EntitiesCheckpoint, ExecInput, ExecOutput, Stage, StageCheckpoint, StageError,
    StageId, UnwindInput, UnwindOutput,
};
use std::{fmt::Debug, ops::Range, sync::mpsc};
use thiserror::Error;
use tracing::*;

/// Maximum amount of transactions to read from disk at one time before we flush their senders to
/// disk. Since each rayon worker will hold at most 100 transactions (`WORKER_CHUNK_SIZE`), we
/// effectively max limit each batch to 1000 channels in memory.
const BATCH_SIZE: usize = 100_000;

/// Maximum number of senders to recover per rayon worker job.
const WORKER_CHUNK_SIZE: usize = 100;

/// The sender recovery stage iterates over existing transactions,
/// recovers the transaction signer and stores them
/// in [`TransactionSenders`][reth_db::tables::TransactionSenders] table.
#[derive(Clone, Debug)]
pub struct SenderRecoveryStage {
    /// The size of inserted items after which the control
    /// flow will be returned to the pipeline for commit
    pub commit_threshold: u64,
}

impl SenderRecoveryStage {
    /// Create new instance of [`SenderRecoveryStage`].
    pub const fn new(config: SenderRecoveryConfig) -> Self {
        Self { commit_threshold: config.commit_threshold }
    }
}

impl Default for SenderRecoveryStage {
    fn default() -> Self {
        Self { commit_threshold: 5_000_000 }
    }
}

impl<Provider> Stage<Provider> for SenderRecoveryStage
where
    Provider: DBProvider<Tx: DbTxMut>
        + BlockReader
        + StaticFileProviderFactory
        + StatsReader
        + PruneCheckpointReader,
{
    /// Return the id of the stage
    fn id(&self) -> StageId {
        StageId::SenderRecovery
    }

    /// Retrieve the range of transactions to iterate over by querying
    /// [`BlockBodyIndices`][reth_db::tables::BlockBodyIndices],
    /// collect transactions within that range, recover signer for each transaction and store
    /// entries in the [`TransactionSenders`][reth_db::tables::TransactionSenders] table.
    fn execute(&mut self, provider: &Provider, input: ExecInput) -> Result<ExecOutput, StageError> {
        if input.target_reached() {
            return Ok(ExecOutput::done(input.checkpoint()))
        }

        let (tx_range, block_range, is_final_range) =
            input.next_block_range_with_transaction_threshold(provider, self.commit_threshold)?;
        let end_block = *block_range.end();

        // No transactions to walk over
        if tx_range.is_empty() {
            info!(target: "sync::stages::sender_recovery", ?tx_range, "Target transaction already reached");
            return Ok(ExecOutput {
                checkpoint: StageCheckpoint::new(end_block)
                    .with_entities_stage_checkpoint(stage_checkpoint(provider)?),
                done: is_final_range,
            })
        }

        // Acquire the cursor for inserting elements
        let mut senders_cursor = provider.tx_ref().cursor_write::<tables::TransactionSenders>()?;

        info!(target: "sync::stages::sender_recovery", ?tx_range, "Recovering senders");

        // Iterate over transactions in batches, recover the senders and append them
        let batch = tx_range
            .clone()
            .step_by(BATCH_SIZE)
            .map(|start| start..std::cmp::min(start + BATCH_SIZE as u64, tx_range.end))
            .collect::<Vec<Range<u64>>>();

        for range in batch {
            recover_range(range, provider, &mut senders_cursor)?;
        }

        Ok(ExecOutput {
            checkpoint: StageCheckpoint::new(end_block)
                .with_entities_stage_checkpoint(stage_checkpoint(provider)?),
            done: is_final_range,
        })
    }

    /// Unwind the stage.
    fn unwind(
        &mut self,
        provider: &Provider,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        let (_, unwind_to, _) = input.unwind_block_range_with_threshold(self.commit_threshold);

        // Lookup latest tx id that we should unwind to
        let latest_tx_id = provider
            .block_body_indices(unwind_to)?
            .ok_or(ProviderError::BlockBodyIndicesNotFound(unwind_to))?
            .last_tx_num();
        provider.tx_ref().unwind_table_by_num::<tables::TransactionSenders>(latest_tx_id)?;

        Ok(UnwindOutput {
            checkpoint: StageCheckpoint::new(unwind_to)
                .with_entities_stage_checkpoint(stage_checkpoint(provider)?),
        })
    }
}

fn recover_range<Provider, CURSOR>(
    tx_range: Range<u64>,
    provider: &Provider,
    senders_cursor: &mut CURSOR,
) -> Result<(), StageError>
where
    Provider: DBProvider + HeaderProvider + StaticFileProviderFactory,
    CURSOR: DbCursorRW<tables::TransactionSenders>,
{
    debug!(target: "sync::stages::sender_recovery", ?tx_range, "Recovering senders batch");

    // Preallocate channels
    let (chunks, receivers): (Vec<_>, Vec<_>) = tx_range
        .clone()
        .step_by(WORKER_CHUNK_SIZE)
        .map(|start| {
            let range = start..std::cmp::min(start + WORKER_CHUNK_SIZE as u64, tx_range.end);
            let (tx, rx) = mpsc::channel();
            // Range and channel sender will be sent to rayon worker
            ((range, tx), rx)
        })
        .unzip();

    let static_file_provider = provider.static_file_provider();

    // We do not use `tokio::task::spawn_blocking` because, during a shutdown,
    // there will be a timeout grace period in which Tokio does not allow spawning
    // additional blocking tasks. This would cause this function to return
    // `SenderRecoveryStageError::RecoveredSendersMismatch` at the end.
    //
    // However, using `std::thread::spawn` allows us to utilize the timeout grace
    // period to complete some work without throwing errors during the shutdown.
    std::thread::spawn(move || {
        for (chunk_range, recovered_senders_tx) in chunks {
            // Read the raw value, and let the rayon worker to decompress & decode.
            let chunk = match static_file_provider.fetch_range_with_predicate(
                StaticFileSegment::Transactions,
                chunk_range.clone(),
                |cursor, number| {
                    Ok(cursor
                        .get_one::<TransactionMask<RawValue<TransactionSignedNoHash>>>(
                            number.into(),
                        )?
                        .map(|tx| (number, tx)))
                },
                |_| true,
            ) {
                Ok(chunk) => chunk,
                Err(err) => {
                    // We exit early since we could not process this chunk.
                    let _ = recovered_senders_tx
                        .send(Err(Box::new(SenderRecoveryStageError::StageError(err.into()))));
                    break
                }
            };

            // Spawn the task onto the global rayon pool
            // This task will send the results through the channel after it has read the transaction
            // and calculated the sender.
            rayon::spawn(move || {
                let mut rlp_buf = Vec::with_capacity(128);
                for (number, tx) in chunk {
                    let res = tx
                        .value()
                        .map_err(|err| Box::new(SenderRecoveryStageError::StageError(err.into())))
                        .and_then(|tx| recover_sender((number, tx), &mut rlp_buf));

                    let is_err = res.is_err();

                    let _ = recovered_senders_tx.send(res);

                    // Finish early
                    if is_err {
                        break
                    }
                }
            });
        }
    });

    debug!(target: "sync::stages::sender_recovery", ?tx_range, "Appending recovered senders to the database");

    let mut processed_transactions = 0;
    for channel in receivers {
        while let Ok(recovered) = channel.recv() {
            let (tx_id, sender) = match recovered {
                Ok(result) => result,
                Err(error) => {
                    return match *error {
                        SenderRecoveryStageError::FailedRecovery(err) => {
                            // get the block number for the bad transaction
                            let block_number = provider
                                .tx_ref()
                                .get::<tables::TransactionBlocks>(err.tx)?
                                .ok_or(ProviderError::BlockNumberForTransactionIndexNotFound)?;

                            // fetch the sealed header so we can use it in the sender recovery
                            // unwind
                            let sealed_header =
                                provider.sealed_header(block_number)?.ok_or_else(|| {
                                    ProviderError::HeaderNotFound(block_number.into())
                                })?;
                            Err(StageError::Block {
                                block: Box::new(sealed_header),
                                error: BlockErrorKind::Validation(
                                    ConsensusError::TransactionSignerRecoveryError,
                                ),
                            })
                        }
                        SenderRecoveryStageError::StageError(err) => Err(err),
                        SenderRecoveryStageError::RecoveredSendersMismatch(expectation) => {
                            Err(StageError::Fatal(
                                SenderRecoveryStageError::RecoveredSendersMismatch(expectation)
                                    .into(),
                            ))
                        }
                    }
                }
            };
            senders_cursor.append(tx_id, sender)?;
            processed_transactions += 1;
        }
    }
    debug!(target: "sync::stages::sender_recovery", ?tx_range, "Finished recovering senders batch");

    // Fail safe to ensure that we do not proceed without having recovered all senders.
    let expected = tx_range.end - tx_range.start;
    if processed_transactions != expected {
        return Err(StageError::Fatal(
            SenderRecoveryStageError::RecoveredSendersMismatch(GotExpected {
                got: processed_transactions,
                expected,
            })
            .into(),
        ));
    }

    Ok(())
}

#[inline]
fn recover_sender(
    (tx_id, tx): (TxNumber, TransactionSignedNoHash),
    rlp_buf: &mut Vec<u8>,
) -> Result<(u64, Address), Box<SenderRecoveryStageError>> {
    // We call [Signature::encode_and_recover_unchecked] because transactions run in the pipeline
    // are known to be valid - this means that we do not need to check whether or not the `s`
    // value is greater than `secp256k1n / 2` if past EIP-2. There are transactions
    // pre-homestead which have large `s` values, so using [Signature::recover_signer] here
    // would not be backwards-compatible.
    let sender = tx
        .encode_and_recover_unchecked(rlp_buf)
        .ok_or(SenderRecoveryStageError::FailedRecovery(FailedSenderRecoveryError { tx: tx_id }))?;

    Ok((tx_id, sender))
}

fn stage_checkpoint<Provider>(provider: &Provider) -> Result<EntitiesCheckpoint, StageError>
where
    Provider: StatsReader + StaticFileProviderFactory + PruneCheckpointReader,
{
    let pruned_entries = provider
        .get_prune_checkpoint(PruneSegment::SenderRecovery)?
        .and_then(|checkpoint| checkpoint.tx_number)
        .unwrap_or_default();
    Ok(EntitiesCheckpoint {
        // If `TransactionSenders` table was pruned, we will have a number of entries in it not
        // matching the actual number of processed transactions. To fix that, we add the
        // number of pruned `TransactionSenders` entries.
        processed: provider.count_entries::<tables::TransactionSenders>()? as u64 + pruned_entries,
        // Count only static files entries. If we count the database entries too, we may have
        // duplicates. We're sure that the static files have all entries that database has,
        // because we run the `StaticFileProducer` before starting the pipeline.
        total: provider.static_file_provider().count_entries::<tables::Transactions>()? as u64,
    })
}

#[derive(Error, Debug)]
#[error(transparent)]
enum SenderRecoveryStageError {
    /// A transaction failed sender recovery
    #[error(transparent)]
    FailedRecovery(#[from] FailedSenderRecoveryError),

    /// Number of recovered senders does not match
    #[error("mismatched sender count during recovery: {_0}")]
    RecoveredSendersMismatch(GotExpected<u64>),

    /// A different type of stage error occurred
    #[error(transparent)]
    StageError(#[from] StageError),
}

#[derive(Error, Debug)]
#[error("sender recovery failed for transaction {tx}")]
struct FailedSenderRecoveryError {
    /// The transaction that failed sender recovery
    tx: TxNumber,
}

#[cfg(test)]
mod tests {
    use alloy_primitives::{BlockNumber, B256};
    use assert_matches::assert_matches;
    use reth_db_api::cursor::DbCursorRO;
    use reth_primitives::{SealedBlock, TransactionSigned};
    use reth_provider::{
        providers::StaticFileWriter, DatabaseProviderFactory, PruneCheckpointWriter,
        StaticFileProviderFactory, TransactionsProvider,
    };
    use reth_prune_types::{PruneCheckpoint, PruneMode};
    use reth_stages_api::StageUnitCheckpoint;
    use reth_testing_utils::generators::{
        self, random_block, random_block_range, BlockParams, BlockRangeParams,
    };

    use super::*;
    use crate::test_utils::{
        stage_test_suite_ext, ExecuteStageTestRunner, StageTestRunner, StorageKind,
        TestRunnerError, TestStageDB, UnwindStageTestRunner,
    };

    stage_test_suite_ext!(SenderRecoveryTestRunner, sender_recovery);

    /// Execute a block range with a single transaction
    #[tokio::test]
    async fn execute_single_transaction() {
        let (previous_stage, stage_progress) = (500, 100);
        let mut rng = generators::rng();

        // Set up the runner
        let runner = SenderRecoveryTestRunner::default();
        let input = ExecInput {
            target: Some(previous_stage),
            checkpoint: Some(StageCheckpoint::new(stage_progress)),
        };

        // Insert blocks with a single transaction at block `stage_progress + 10`
        let non_empty_block_number = stage_progress + 10;
        let blocks = (stage_progress..=input.target())
            .map(|number| {
                random_block(
                    &mut rng,
                    number,
                    BlockParams {
                        tx_count: Some((number == non_empty_block_number) as u8),
                        ..Default::default()
                    },
                )
            })
            .collect::<Vec<_>>();
        runner
            .db
            .insert_blocks(blocks.iter(), StorageKind::Static)
            .expect("failed to insert blocks");

        let rx = runner.execute(input);

        // Assert the successful result
        let result = rx.await.unwrap();
        assert_matches!(
            result,
            Ok(ExecOutput { checkpoint: StageCheckpoint {
                block_number,
                stage_checkpoint: Some(StageUnitCheckpoint::Entities(EntitiesCheckpoint {
                    processed: 1,
                    total: 1
                }))
            }, done: true }) if block_number == previous_stage
        );

        // Validate the stage execution
        assert!(runner.validate_execution(input, result.ok()).is_ok(), "execution validation");
    }

    /// Execute the stage twice with input range that exceeds the commit threshold
    #[tokio::test]
    async fn execute_intermediate_commit() {
        let mut rng = generators::rng();

        let threshold = 10;
        let mut runner = SenderRecoveryTestRunner::default();
        runner.set_threshold(threshold);
        let (stage_progress, previous_stage) = (1000, 1100); // input exceeds threshold

        // Manually seed once with full input range
        let seed = random_block_range(
            &mut rng,
            stage_progress + 1..=previous_stage,
            BlockRangeParams { parent: Some(B256::ZERO), tx_count: 0..4, ..Default::default() },
        ); // set tx count range high enough to hit the threshold
        runner
            .db
            .insert_blocks(seed.iter(), StorageKind::Static)
            .expect("failed to seed execution");

        let total_transactions = runner
            .db
            .factory
            .static_file_provider()
            .count_entries::<tables::Transactions>()
            .unwrap() as u64;

        let first_input = ExecInput {
            target: Some(previous_stage),
            checkpoint: Some(StageCheckpoint::new(stage_progress)),
        };

        // Execute first time
        let result = runner.execute(first_input).await.unwrap();
        let mut tx_count = 0;
        let expected_progress = seed
            .iter()
            .find(|x| {
                tx_count += x.body.transactions.len();
                tx_count as u64 > threshold
            })
            .map(|x| x.number)
            .unwrap_or(previous_stage);
        assert_matches!(result, Ok(_));
        assert_eq!(
            result.unwrap(),
            ExecOutput {
                checkpoint: StageCheckpoint::new(expected_progress).with_entities_stage_checkpoint(
                    EntitiesCheckpoint {
                        processed: runner.db.table::<tables::TransactionSenders>().unwrap().len()
                            as u64,
                        total: total_transactions
                    }
                ),
                done: false
            }
        );

        // Execute second time to completion
        runner.set_threshold(u64::MAX);
        let second_input = ExecInput {
            target: Some(previous_stage),
            checkpoint: Some(StageCheckpoint::new(expected_progress)),
        };
        let result = runner.execute(second_input).await.unwrap();
        assert_matches!(result, Ok(_));
        assert_eq!(
            result.as_ref().unwrap(),
            &ExecOutput {
                checkpoint: StageCheckpoint::new(previous_stage).with_entities_stage_checkpoint(
                    EntitiesCheckpoint { processed: total_transactions, total: total_transactions }
                ),
                done: true
            }
        );

        assert!(runner.validate_execution(first_input, result.ok()).is_ok(), "validation failed");
    }

    #[test]
    fn stage_checkpoint_pruned() {
        let db = TestStageDB::default();
        let mut rng = generators::rng();

        let blocks = random_block_range(
            &mut rng,
            0..=100,
            BlockRangeParams { parent: Some(B256::ZERO), tx_count: 0..10, ..Default::default() },
        );
        db.insert_blocks(blocks.iter(), StorageKind::Static).expect("insert blocks");

        let max_pruned_block = 30;
        let max_processed_block = 70;

        let mut tx_senders = Vec::new();
        let mut tx_number = 0;
        for block in &blocks[..=max_processed_block] {
            for transaction in &block.body.transactions {
                if block.number > max_pruned_block {
                    tx_senders
                        .push((tx_number, transaction.recover_signer().expect("recover signer")));
                }
                tx_number += 1;
            }
        }
        db.insert_transaction_senders(tx_senders).expect("insert tx hash numbers");

        let provider = db.factory.provider_rw().unwrap();
        provider
            .save_prune_checkpoint(
                PruneSegment::SenderRecovery,
                PruneCheckpoint {
                    block_number: Some(max_pruned_block),
                    tx_number: Some(
                        blocks[..=max_pruned_block as usize]
                            .iter()
                            .map(|block| block.body.transactions.len() as u64)
                            .sum::<u64>(),
                    ),
                    prune_mode: PruneMode::Full,
                },
            )
            .expect("save stage checkpoint");
        provider.commit().expect("commit");

        let provider = db.factory.database_provider_rw().unwrap();
        assert_eq!(
            stage_checkpoint(&provider).expect("stage checkpoint"),
            EntitiesCheckpoint {
                processed: blocks[..=max_processed_block]
                    .iter()
                    .map(|block| block.body.transactions.len() as u64)
                    .sum::<u64>(),
                total: blocks.iter().map(|block| block.body.transactions.len() as u64).sum::<u64>()
            }
        );
    }

    struct SenderRecoveryTestRunner {
        db: TestStageDB,
        threshold: u64,
    }

    impl Default for SenderRecoveryTestRunner {
        fn default() -> Self {
            Self { threshold: 1000, db: TestStageDB::default() }
        }
    }

    impl SenderRecoveryTestRunner {
        fn set_threshold(&mut self, threshold: u64) {
            self.threshold = threshold;
        }

        /// # Panics
        ///
        /// 1. If there are any entries in the [`tables::TransactionSenders`] table above a given
        ///    block number.
        /// 2. If the is no requested block entry in the bodies table, but
        ///    [`tables::TransactionSenders`] is not empty.
        fn ensure_no_senders_by_block(&self, block: BlockNumber) -> Result<(), TestRunnerError> {
            let body_result = self
                .db
                .factory
                .provider_rw()?
                .block_body_indices(block)?
                .ok_or(ProviderError::BlockBodyIndicesNotFound(block));
            match body_result {
                Ok(body) => self.db.ensure_no_entry_above::<tables::TransactionSenders, _>(
                    body.last_tx_num(),
                    |key| key,
                )?,
                Err(_) => {
                    assert!(self.db.table_is_empty::<tables::TransactionSenders>()?);
                }
            };

            Ok(())
        }
    }

    impl StageTestRunner for SenderRecoveryTestRunner {
        type S = SenderRecoveryStage;

        fn db(&self) -> &TestStageDB {
            &self.db
        }

        fn stage(&self) -> Self::S {
            SenderRecoveryStage { commit_threshold: self.threshold }
        }
    }

    impl ExecuteStageTestRunner for SenderRecoveryTestRunner {
        type Seed = Vec<SealedBlock>;

        fn seed_execution(&mut self, input: ExecInput) -> Result<Self::Seed, TestRunnerError> {
            let mut rng = generators::rng();
            let stage_progress = input.checkpoint().block_number;
            let end = input.target();

            let blocks = random_block_range(
                &mut rng,
                stage_progress..=end,
                BlockRangeParams { parent: Some(B256::ZERO), tx_count: 0..2, ..Default::default() },
            );
            self.db.insert_blocks(blocks.iter(), StorageKind::Static)?;
            Ok(blocks)
        }

        fn validate_execution(
            &self,
            input: ExecInput,
            output: Option<ExecOutput>,
        ) -> Result<(), TestRunnerError> {
            match output {
                Some(output) => {
                    let provider = self.db.factory.provider()?;
                    let start_block = input.next_block();
                    let end_block = output.checkpoint.block_number;

                    if start_block > end_block {
                        return Ok(())
                    }

                    let mut body_cursor =
                        provider.tx_ref().cursor_read::<tables::BlockBodyIndices>()?;
                    body_cursor.seek_exact(start_block)?;

                    while let Some((_, body)) = body_cursor.next()? {
                        for tx_id in body.tx_num_range() {
                            let transaction: TransactionSigned = provider
                                .transaction_by_id_no_hash(tx_id)?
                                .map(|tx| TransactionSigned {
                                    hash: Default::default(), // we don't require the hash
                                    signature: tx.signature,
                                    transaction: tx.transaction,
                                })
                                .expect("no transaction entry");
                            let signer =
                                transaction.recover_signer().expect("failed to recover signer");
                            assert_eq!(Some(signer), provider.transaction_sender(tx_id)?)
                        }
                    }
                }
                None => self.ensure_no_senders_by_block(input.checkpoint().block_number)?,
            };

            Ok(())
        }
    }

    impl UnwindStageTestRunner for SenderRecoveryTestRunner {
        fn validate_unwind(&self, input: UnwindInput) -> Result<(), TestRunnerError> {
            self.ensure_no_senders_by_block(input.unwind_to)
        }
    }
}
