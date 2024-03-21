use crate::{ExecInput, ExecOutput, Stage, StageError, UnwindInput, UnwindOutput};
use num_traits::Zero;
use reth_config::config::EtlConfig;
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW},
    database::Database,
    tables,
    transaction::{DbTx, DbTxMut},
    RawKey, RawValue,
};
use reth_etl::Collector;
use reth_interfaces::provider::ProviderError;
use reth_primitives::{
    stage::{EntitiesCheckpoint, StageCheckpoint, StageId},
    PruneCheckpoint, PruneMode, PrunePurpose, PruneSegment, TxHash, TxNumber,
};
use reth_provider::{
    BlockReader, DatabaseProviderRW, PruneCheckpointReader, PruneCheckpointWriter, StatsReader,
    TransactionsProvider, TransactionsProviderExt,
};
use tracing::*;

/// The transaction lookup stage.
///
/// This stage walks over existing transactions, and sets the transaction hash of each transaction
/// in a block to the corresponding `BlockNumber` at each block. This is written to the
/// [`tables::TransactionHashNumbers`] This is used for looking up changesets via the transaction
/// hash.
///
/// It uses [`reth_etl::Collector`] to collect all entries before finally writing them to disk.
#[derive(Debug, Clone)]
pub struct TransactionLookupStage {
    /// The maximum number of lookup entries to hold in memory before pushing them to
    /// [`reth_etl::Collector`].
    chunk_size: u64,
    etl_config: EtlConfig,
    prune_mode: Option<PruneMode>,
}

impl Default for TransactionLookupStage {
    fn default() -> Self {
        Self { chunk_size: 5_000_000, etl_config: EtlConfig::default(), prune_mode: None }
    }
}

impl TransactionLookupStage {
    /// Create new instance of [TransactionLookupStage].
    pub fn new(chunk_size: u64, etl_config: EtlConfig, prune_mode: Option<PruneMode>) -> Self {
        Self { chunk_size, etl_config, prune_mode }
    }
}

impl<DB: Database> Stage<DB> for TransactionLookupStage {
    /// Return the id of the stage
    fn id(&self) -> StageId {
        StageId::TransactionLookup
    }

    /// Write transaction hash -> id entries
    fn execute(
        &mut self,
        provider: &DatabaseProviderRW<DB>,
        mut input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        if let Some((target_prunable_block, prune_mode)) = self
            .prune_mode
            .map(|mode| {
                mode.prune_target_block(
                    input.target(),
                    PruneSegment::TransactionLookup,
                    PrunePurpose::User,
                )
            })
            .transpose()?
            .flatten()
        {
            if target_prunable_block > input.checkpoint().block_number {
                input.checkpoint = Some(StageCheckpoint::new(target_prunable_block));

                // Save prune checkpoint only if we don't have one already.
                // Otherwise, pruner may skip the unpruned range of blocks.
                if provider.get_prune_checkpoint(PruneSegment::TransactionLookup)?.is_none() {
                    let target_prunable_tx_number = provider
                        .block_body_indices(target_prunable_block)?
                        .ok_or(ProviderError::BlockBodyIndicesNotFound(target_prunable_block))?
                        .last_tx_num();

                    provider.save_prune_checkpoint(
                        PruneSegment::TransactionLookup,
                        PruneCheckpoint {
                            block_number: Some(target_prunable_block),
                            tx_number: Some(target_prunable_tx_number),
                            prune_mode,
                        },
                    )?;
                }
            }
        }
        if input.target_reached() {
            return Ok(ExecOutput::done(input.checkpoint()))
        }

        // 500MB temporary files
        let mut hash_collector: Collector<TxHash, TxNumber> =
            Collector::new(self.etl_config.file_size, self.etl_config.dir.clone());

        debug!(
            target: "sync::stages::transaction_lookup",
            tx_range = ?input.checkpoint().block_number..=input.target(),
            "Updating transaction lookup"
        );

        loop {
            let (tx_range, block_range, is_final_range) =
                input.next_block_range_with_transaction_threshold(provider, self.chunk_size)?;

            let end_block = *block_range.end();

            debug!(target: "sync::stages::transaction_lookup", ?tx_range, "Calculating transaction hashes");

            for (key, value) in provider.transaction_hashes_by_range(tx_range)? {
                hash_collector.insert(key, value)?;
            }

            input.checkpoint = Some(
                StageCheckpoint::new(end_block)
                    .with_entities_stage_checkpoint(stage_checkpoint(provider)?),
            );

            if is_final_range {
                let append_only =
                    provider.count_entries::<tables::TransactionHashNumbers>()?.is_zero();
                let mut txhash_cursor = provider
                    .tx_ref()
                    .cursor_write::<tables::RawTable<tables::TransactionHashNumbers>>()?;

                let total_hashes = hash_collector.len();
                let interval = (total_hashes / 10).max(1);
                for (index, hash_to_number) in hash_collector.iter()?.enumerate() {
                    let (hash, number) = hash_to_number?;
                    if index > 0 && index % interval == 0 {
                        debug!(
                            target: "sync::stages::transaction_lookup",
                            ?append_only,
                            progress = format!("{:.2}%", (index as f64 / total_hashes as f64) * 100.0),
                            "Inserting hashes"
                        );
                    }

                    if append_only {
                        txhash_cursor.append(
                            RawKey::<TxHash>::from_vec(hash),
                            RawValue::<TxNumber>::from_vec(number),
                        )?;
                    } else {
                        txhash_cursor.insert(
                            RawKey::<TxHash>::from_vec(hash),
                            RawValue::<TxNumber>::from_vec(number),
                        )?;
                    }
                }
                break
            }
        }

        Ok(ExecOutput {
            checkpoint: StageCheckpoint::new(input.target())
                .with_entities_stage_checkpoint(stage_checkpoint(provider)?),
            done: true,
        })
    }

    /// Unwind the stage.
    fn unwind(
        &mut self,
        provider: &DatabaseProviderRW<DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        let tx = provider.tx_ref();
        let (range, unwind_to, _) = input.unwind_block_range_with_threshold(self.chunk_size);

        // Cursors to unwind tx hash to number
        let mut body_cursor = tx.cursor_read::<tables::BlockBodyIndices>()?;
        let mut tx_hash_number_cursor = tx.cursor_write::<tables::TransactionHashNumbers>()?;
        let static_file_provider = provider.static_file_provider();
        let mut rev_walker = body_cursor.walk_back(Some(*range.end()))?;
        while let Some((number, body)) = rev_walker.next().transpose()? {
            if number <= unwind_to {
                break
            }

            // Delete all transactions that belong to this block
            for tx_id in body.tx_num_range() {
                // First delete the transaction and hash to id mapping
                if let Some(transaction) = static_file_provider.transaction_by_id(tx_id)? {
                    if tx_hash_number_cursor.seek_exact(transaction.hash())?.is_some() {
                        tx_hash_number_cursor.delete_current()?;
                    }
                }
            }
        }

        Ok(UnwindOutput {
            checkpoint: StageCheckpoint::new(unwind_to)
                .with_entities_stage_checkpoint(stage_checkpoint(provider)?),
        })
    }
}

fn stage_checkpoint<DB: Database>(
    provider: &DatabaseProviderRW<DB>,
) -> Result<EntitiesCheckpoint, StageError> {
    let pruned_entries = provider
        .get_prune_checkpoint(PruneSegment::TransactionLookup)?
        .and_then(|checkpoint| checkpoint.tx_number)
        // `+1` is needed because `TxNumber` is 0-indexed
        .map(|tx_number| tx_number + 1)
        .unwrap_or_default();
    Ok(EntitiesCheckpoint {
        // If `TransactionHashNumbers` table was pruned, we will have a number of entries in it not
        // matching the actual number of processed transactions. To fix that, we add the
        // number of pruned `TransactionHashNumbers` entries.
        processed: provider.count_entries::<tables::TransactionHashNumbers>()? as u64 +
            pruned_entries,
        // Count only static files entries. If we count the database entries too, we may have
        // duplicates. We're sure that the static files have all entries that database has,
        // because we run the `StaticFileProducer` before starting the pipeline.
        total: provider.static_file_provider().count_entries::<tables::Transactions>()? as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{
        stage_test_suite_ext, ExecuteStageTestRunner, StageTestRunner, StorageKind,
        TestRunnerError, TestStageDB, UnwindStageTestRunner,
    };
    use assert_matches::assert_matches;
    use reth_interfaces::test_utils::{
        generators,
        generators::{random_block, random_block_range},
    };
    use reth_primitives::{stage::StageUnitCheckpoint, BlockNumber, SealedBlock, B256};
    use reth_provider::providers::StaticFileWriter;
    use std::ops::Sub;

    // Implement stage test suite.
    stage_test_suite_ext!(TransactionLookupTestRunner, transaction_lookup);

    #[tokio::test]
    async fn execute_single_transaction_lookup() {
        let (previous_stage, stage_progress) = (500, 100);
        let mut rng = generators::rng();

        // Set up the runner
        let runner = TransactionLookupTestRunner::default();
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
                    None,
                    Some((number == non_empty_block_number) as u8),
                    None,
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
            Ok(ExecOutput {
                checkpoint: StageCheckpoint {
                block_number,
                stage_checkpoint: Some(StageUnitCheckpoint::Entities(EntitiesCheckpoint {
                    processed,
                    total
                }))
            }, done: true }) if block_number == previous_stage && processed == total &&
                total == runner.db.factory.static_file_provider().count_entries::<tables::Transactions>().unwrap() as u64
        );

        // Validate the stage execution
        assert!(runner.validate_execution(input, result.ok()).is_ok(), "execution validation");
    }

    #[tokio::test]
    async fn execute_pruned_transaction_lookup() {
        let (previous_stage, prune_target, stage_progress) = (500, 400, 100);
        let mut rng = generators::rng();

        // Set up the runner
        let mut runner = TransactionLookupTestRunner::default();
        let input = ExecInput {
            target: Some(previous_stage),
            checkpoint: Some(StageCheckpoint::new(stage_progress)),
        };

        // Seed only once with full input range
        let seed =
            random_block_range(&mut rng, stage_progress + 1..=previous_stage, B256::ZERO, 0..2);
        runner
            .db
            .insert_blocks(seed.iter(), StorageKind::Static)
            .expect("failed to seed execution");

        runner.set_prune_mode(PruneMode::Before(prune_target));

        let rx = runner.execute(input);

        // Assert the successful result
        let result = rx.await.unwrap();
        assert_matches!(
            result,
            Ok(ExecOutput {
                checkpoint: StageCheckpoint {
                block_number,
                stage_checkpoint: Some(StageUnitCheckpoint::Entities(EntitiesCheckpoint {
                    processed,
                    total
                }))
            }, done: true }) if block_number == previous_stage && processed == total &&
                total == runner.db.factory.static_file_provider().count_entries::<tables::Transactions>().unwrap() as u64
        );

        // Validate the stage execution
        assert!(runner.validate_execution(input, result.ok()).is_ok(), "execution validation");
    }

    #[test]
    fn stage_checkpoint_pruned() {
        let db = TestStageDB::default();
        let mut rng = generators::rng();

        let blocks = random_block_range(&mut rng, 0..=100, B256::ZERO, 0..10);
        db.insert_blocks(blocks.iter(), StorageKind::Static).expect("insert blocks");

        let max_pruned_block = 30;
        let max_processed_block = 70;

        let mut tx_hash_numbers = Vec::new();
        let mut tx_hash_number = 0;
        for block in &blocks[..=max_processed_block] {
            for transaction in &block.body {
                if block.number > max_pruned_block {
                    tx_hash_numbers.push((transaction.hash, tx_hash_number));
                }
                tx_hash_number += 1;
            }
        }
        db.insert_tx_hash_numbers(tx_hash_numbers).expect("insert tx hash numbers");

        let provider = db.factory.provider_rw().unwrap();
        provider
            .save_prune_checkpoint(
                PruneSegment::TransactionLookup,
                PruneCheckpoint {
                    block_number: Some(max_pruned_block),
                    tx_number: Some(
                        blocks[..=max_pruned_block as usize]
                            .iter()
                            .map(|block| block.body.len() as u64)
                            .sum::<u64>()
                            .sub(1), // `TxNumber` is 0-indexed
                    ),
                    prune_mode: PruneMode::Full,
                },
            )
            .expect("save stage checkpoint");
        provider.commit().expect("commit");

        let provider = db.factory.provider_rw().unwrap();
        assert_eq!(
            stage_checkpoint(&provider).expect("stage checkpoint"),
            EntitiesCheckpoint {
                processed: blocks[..=max_processed_block]
                    .iter()
                    .map(|block| block.body.len() as u64)
                    .sum::<u64>(),
                total: blocks.iter().map(|block| block.body.len() as u64).sum::<u64>()
            }
        );
    }

    struct TransactionLookupTestRunner {
        db: TestStageDB,
        chunk_size: u64,
        etl_config: EtlConfig,
        prune_mode: Option<PruneMode>,
    }

    impl Default for TransactionLookupTestRunner {
        fn default() -> Self {
            Self {
                db: TestStageDB::default(),
                chunk_size: 1000,
                etl_config: EtlConfig::default(),
                prune_mode: None,
            }
        }
    }

    impl TransactionLookupTestRunner {
        fn set_prune_mode(&mut self, prune_mode: PruneMode) {
            self.prune_mode = Some(prune_mode);
        }

        /// # Panics
        ///
        /// 1. If there are any entries in the [tables::TransactionHashNumbers] table above a given
        /// block    number.
        ///
        /// 2. If the is no requested block entry in the bodies table, but
        /// [tables::TransactionHashNumbers] is    not empty.
        fn ensure_no_hash_by_block(&self, number: BlockNumber) -> Result<(), TestRunnerError> {
            let body_result = self
                .db
                .factory
                .provider_rw()?
                .block_body_indices(number)?
                .ok_or(ProviderError::BlockBodyIndicesNotFound(number));
            match body_result {
                Ok(body) => {
                    self.db.ensure_no_entry_above_by_value::<tables::TransactionHashNumbers, _>(
                        body.last_tx_num(),
                        |key| key,
                    )?
                }
                Err(_) => {
                    assert!(self.db.table_is_empty::<tables::TransactionHashNumbers>()?);
                }
            };

            Ok(())
        }
    }

    impl StageTestRunner for TransactionLookupTestRunner {
        type S = TransactionLookupStage;

        fn db(&self) -> &TestStageDB {
            &self.db
        }

        fn stage(&self) -> Self::S {
            TransactionLookupStage {
                chunk_size: self.chunk_size,
                etl_config: self.etl_config.clone(),
                prune_mode: self.prune_mode,
            }
        }
    }

    impl ExecuteStageTestRunner for TransactionLookupTestRunner {
        type Seed = Vec<SealedBlock>;

        fn seed_execution(&mut self, input: ExecInput) -> Result<Self::Seed, TestRunnerError> {
            let stage_progress = input.checkpoint().block_number;
            let end = input.target();
            let mut rng = generators::rng();

            let blocks = random_block_range(&mut rng, stage_progress + 1..=end, B256::ZERO, 0..2);
            self.db.insert_blocks(blocks.iter(), StorageKind::Static)?;
            Ok(blocks)
        }

        fn validate_execution(
            &self,
            mut input: ExecInput,
            output: Option<ExecOutput>,
        ) -> Result<(), TestRunnerError> {
            match output {
                Some(output) => {
                    let provider = self.db.factory.provider()?;

                    if let Some((target_prunable_block, _)) = self
                        .prune_mode
                        .map(|mode| {
                            mode.prune_target_block(
                                input.target(),
                                PruneSegment::TransactionLookup,
                                PrunePurpose::User,
                            )
                        })
                        .transpose()
                        .expect("prune target block for transaction lookup")
                        .flatten()
                    {
                        if target_prunable_block > input.checkpoint().block_number {
                            input.checkpoint = Some(StageCheckpoint::new(target_prunable_block));
                        }
                    }
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
                            let transaction =
                                provider.transaction_by_id(tx_id)?.expect("no transaction entry");
                            assert_eq!(Some(tx_id), provider.transaction_id(transaction.hash())?);
                        }
                    }
                }
                None => self.ensure_no_hash_by_block(input.checkpoint().block_number)?,
            };
            Ok(())
        }
    }

    impl UnwindStageTestRunner for TransactionLookupTestRunner {
        fn validate_unwind(&self, input: UnwindInput) -> Result<(), TestRunnerError> {
            self.ensure_no_hash_by_block(input.unwind_to)
        }
    }
}
