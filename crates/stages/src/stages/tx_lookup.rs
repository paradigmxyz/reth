use crate::{ExecInput, ExecOutput, Stage, StageError, UnwindInput, UnwindOutput};
use itertools::Itertools;
use rayon::prelude::*;
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW},
    database::Database,
    tables,
    transaction::{DbTx, DbTxMut},
    DatabaseError,
};
use reth_primitives::{
    rpc_utils::keccak256,
    stage::{EntitiesCheckpoint, StageCheckpoint, StageId},
    TransactionSignedNoHash, TxNumber, H256,
};
use reth_provider::DatabaseProviderRW;
use tokio::sync::mpsc;
use tracing::*;

/// The transaction lookup stage.
///
/// This stage walks over the bodies table, and sets the transaction hash of each transaction in a
/// block to the corresponding `BlockNumber` at each block. This is written to the
/// [`tables::TxHashNumber`] This is used for looking up changesets via the transaction hash.
#[derive(Debug, Clone)]
pub struct TransactionLookupStage {
    /// The number of lookup entries to commit at once
    commit_threshold: u64,
}

impl Default for TransactionLookupStage {
    fn default() -> Self {
        Self { commit_threshold: 5_000_000 }
    }
}

impl TransactionLookupStage {
    /// Create new instance of [TransactionLookupStage].
    pub fn new(commit_threshold: u64) -> Self {
        Self { commit_threshold }
    }
}

#[async_trait::async_trait]
impl<DB: Database> Stage<DB> for TransactionLookupStage {
    /// Return the id of the stage
    fn id(&self) -> StageId {
        StageId::TransactionLookup
    }

    /// Write transaction hash -> id entries
    async fn execute(
        &mut self,
        provider: &mut DatabaseProviderRW<'_, &DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        let (tx_range, block_range) =
            input.next_block_range_with_transaction_threshold(provider, self.commit_threshold)?;
        let end_block = *block_range.end();

        debug!(target: "sync::stages::transaction_lookup", ?tx_range, "Updating transaction lookup");

        let tx = provider.tx_ref();
        let mut tx_cursor = tx.cursor_read::<tables::Transactions>()?;
        let tx_walker = tx_cursor.walk_range(tx_range)?;

        let chunk_size = 100_000 / rayon::current_num_threads();
        let mut channels = Vec::with_capacity(chunk_size);
        let mut transaction_count = 0;

        for chunk in &tx_walker.chunks(chunk_size) {
            let (tx, rx) = mpsc::unbounded_channel();
            channels.push(rx);

            // Note: Unfortunate side-effect of how chunk is designed in itertools (it is not Send)
            let chunk: Vec<_> = chunk.collect();
            transaction_count += chunk.len();

            // closure that will calculate the TxHash
            let calculate_hash =
                |entry: Result<(TxNumber, TransactionSignedNoHash), reth_db::DatabaseError>,
                 rlp_buf: &mut Vec<u8>|
                 -> Result<(H256, u64), Box<StageError>> {
                    let (tx_id, tx) = entry.map_err(|e| Box::new(e.into()))?;
                    tx.transaction.encode_with_signature(&tx.signature, rlp_buf, false);
                    Ok((H256(keccak256(rlp_buf)), tx_id))
                };

            // Spawn the task onto the global rayon pool
            // This task will send the results through the channel after it has calculated the hash.
            rayon::spawn(move || {
                let mut rlp_buf = Vec::with_capacity(128);
                for entry in chunk {
                    rlp_buf.clear();
                    let _ = tx.send(calculate_hash(entry, &mut rlp_buf));
                }
            });
        }

        let mut tx_list = Vec::with_capacity(transaction_count);

        // Iterate over channels and append the tx hashes to be sorted out later
        for mut channel in channels {
            while let Some(tx) = channel.recv().await {
                let (tx_hash, tx_id) = tx.map_err(|boxed| *boxed)?;
                tx_list.push((tx_hash, tx_id));
            }
        }

        // Sort before inserting the reverse lookup for hash -> tx_id.
        tx_list.par_sort_unstable_by(|txa, txb| txa.0.cmp(&txb.0));

        let mut txhash_cursor = tx.cursor_write::<tables::TxHashNumber>()?;

        // If the last inserted element in the database is equal or bigger than the first
        // in our set, then we need to insert inside the DB. If it is smaller then last
        // element in the DB, we can append to the DB.
        // Append probably only ever happens during sync, on the first table insertion.
        let insert = tx_list
            .first()
            .zip(txhash_cursor.last()?)
            .map(|((first, _), (last, _))| first <= &last)
            .unwrap_or_default();
        // if txhash_cursor.last() is None we will do insert. `zip` would return none if any item is
        // none. if it is some and if first is smaller than last, we will do append.

        for (tx_hash, id) in tx_list {
            if insert {
                txhash_cursor.insert(tx_hash, id)?;
            } else {
                txhash_cursor.append(tx_hash, id)?;
            }
        }

        Ok(ExecOutput {
            checkpoint: StageCheckpoint::new(end_block)
                .with_entities_stage_checkpoint(stage_checkpoint(provider)?),
        })
    }

    /// Unwind the stage.
    async fn unwind(
        &mut self,
        provider: &mut DatabaseProviderRW<'_, &DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        let tx = provider.tx_ref();
        let (range, unwind_to) = input.unwind_block_range_with_threshold(self.commit_threshold);

        // Cursors to unwind tx hash to number
        let mut body_cursor = tx.cursor_read::<tables::BlockBodyIndices>()?;
        let mut tx_hash_number_cursor = tx.cursor_write::<tables::TxHashNumber>()?;
        let mut transaction_cursor = tx.cursor_read::<tables::Transactions>()?;
        let mut rev_walker = body_cursor.walk_back(Some(*range.end()))?;
        while let Some((number, body)) = rev_walker.next().transpose()? {
            if number <= unwind_to {
                break
            }

            // Delete all transactions that belong to this block
            for tx_id in body.tx_num_range() {
                // First delete the transaction and hash to id mapping
                if let Some((_, transaction)) = transaction_cursor.seek_exact(tx_id)? {
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
    provider: &DatabaseProviderRW<'_, &DB>,
) -> Result<EntitiesCheckpoint, DatabaseError> {
    Ok(EntitiesCheckpoint {
        processed: provider.tx_ref().entries::<tables::TxHashNumber>()? as u64,
        total: provider.tx_ref().entries::<tables::Transactions>()? as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{
        ExecuteStageTestRunner, StageTestRunner, TestRunnerError, TestTransaction,
        UnwindStageTestRunner,
    };
    use assert_matches::assert_matches;
    use reth_interfaces::test_utils::generators::{random_block, random_block_range};
    use reth_primitives::{stage::StageUnitCheckpoint, BlockNumber, SealedBlock, H256};

    #[tokio::test]
    async fn execute_single_transaction_lookup() {
        let (previous_stage, stage_progress) = (500, 100);

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
                random_block(number, None, Some((number == non_empty_block_number) as u8), None)
            })
            .collect::<Vec<_>>();
        runner.tx.insert_blocks(blocks.iter(), None).expect("failed to insert blocks");

        let rx = runner.execute(input);

        // Assert the successful result
        let result = rx.await.unwrap();
        assert_matches!(
            result,
            Ok(ExecOutput {checkpoint: StageCheckpoint {
                block_number,
                stage_checkpoint: Some(StageUnitCheckpoint::Entities(EntitiesCheckpoint {
                    processed,
                    total
                }))
            }}) if block_number == previous_stage && processed == total &&
                total == runner.tx.table::<tables::Transactions>().unwrap().len() as u64
        );

        // Validate the stage execution
        assert!(runner.validate_execution(input, result.ok()).is_ok(), "execution validation");
    }

    /// Execute the stage twice with input range that exceeds the commit threshold
    #[tokio::test]
    async fn execute_intermediate_commit_transaction_lookup() {
        let threshold = 50;
        let mut runner = TransactionLookupTestRunner::default();
        runner.set_threshold(threshold);
        let (stage_progress, previous_stage) = (1000, 1100); // input exceeds threshold
        let first_input = ExecInput {
            target: Some(previous_stage),
            checkpoint: Some(StageCheckpoint::new(stage_progress)),
        };

        // Seed only once with full input range
        let seed = random_block_range(stage_progress + 1..=previous_stage, H256::zero(), 0..4); // set tx count range high enough to hit the threshold
        runner.tx.insert_blocks(seed.iter(), None).expect("failed to seed execution");

        let total_txs = runner.tx.table::<tables::Transactions>().unwrap().len() as u64;

        // Execute first time
        let result = runner.execute(first_input).await.unwrap();
        let mut tx_count = 0;
        let expected_progress = seed
            .iter()
            .find(|x| {
                tx_count += x.body.len();
                tx_count as u64 > threshold
            })
            .map(|x| x.number)
            .unwrap_or(previous_stage);
        assert_matches!(result, Ok(_));
        assert_eq!(
            result.as_ref().unwrap(),
            &ExecOutput {
                checkpoint: StageCheckpoint::new(expected_progress).with_entities_stage_checkpoint(
                    EntitiesCheckpoint {
                        processed: runner.tx.table::<tables::TxHashNumber>().unwrap().len() as u64,
                        total: total_txs
                    }
                )
            }
        );
        assert!(!result.unwrap().is_done(first_input));

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
                    EntitiesCheckpoint { processed: total_txs, total: total_txs }
                )
            }
        );

        assert!(runner.validate_execution(first_input, result.ok()).is_ok(), "validation failed");
    }

    struct TransactionLookupTestRunner {
        tx: TestTransaction,
        threshold: u64,
    }

    impl Default for TransactionLookupTestRunner {
        fn default() -> Self {
            Self { threshold: 1000, tx: TestTransaction::default() }
        }
    }

    impl TransactionLookupTestRunner {
        fn set_threshold(&mut self, threshold: u64) {
            self.threshold = threshold;
        }

        /// # Panics
        ///
        /// 1. If there are any entries in the [tables::TxHashNumber] table above
        ///    a given block number.
        ///
        /// 2. If the is no requested block entry in the bodies table,
        ///    but [tables::TxHashNumber] is not empty.
        fn ensure_no_hash_by_block(&self, number: BlockNumber) -> Result<(), TestRunnerError> {
            let body_result = self.tx.inner().block_body_indices(number);
            match body_result {
                Ok(body) => self.tx.ensure_no_entry_above_by_value::<tables::TxHashNumber, _>(
                    body.last_tx_num(),
                    |key| key,
                )?,
                Err(_) => {
                    assert!(self.tx.table_is_empty::<tables::TxHashNumber>()?);
                }
            };

            Ok(())
        }
    }

    impl StageTestRunner for TransactionLookupTestRunner {
        type S = TransactionLookupStage;

        fn tx(&self) -> &TestTransaction {
            &self.tx
        }

        fn stage(&self) -> Self::S {
            TransactionLookupStage { commit_threshold: self.threshold }
        }
    }

    impl ExecuteStageTestRunner for TransactionLookupTestRunner {
        type Seed = Vec<SealedBlock>;

        fn seed_execution(&mut self, input: ExecInput) -> Result<Self::Seed, TestRunnerError> {
            let stage_progress = input.checkpoint().block_number;
            let end = input.target();

            let blocks = random_block_range(stage_progress + 1..=end, H256::zero(), 0..2);
            self.tx.insert_blocks(blocks.iter(), None)?;
            Ok(blocks)
        }

        fn validate_execution(
            &self,
            input: ExecInput,
            output: Option<ExecOutput>,
        ) -> Result<(), TestRunnerError> {
            match output {
                Some(output) => self.tx.query(|tx| {
                    let start_block = input.next_block();
                    let end_block = output.checkpoint.block_number;

                    if start_block > end_block {
                        return Ok(())
                    }

                    let mut body_cursor = tx.cursor_read::<tables::BlockBodyIndices>()?;
                    body_cursor.seek_exact(start_block)?;

                    while let Some((_, body)) = body_cursor.next()? {
                        for tx_id in body.tx_num_range() {
                            let transaction = tx
                                .get::<tables::Transactions>(tx_id)?
                                .expect("no transaction entry");
                            assert_eq!(
                                Some(tx_id),
                                tx.get::<tables::TxHashNumber>(transaction.hash())?,
                            );
                        }
                    }

                    Ok(())
                })?,
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
