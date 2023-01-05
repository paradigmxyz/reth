use crate::{
    db::Transaction, ExecInput, ExecOutput, Stage, StageError, StageId, UnwindInput, UnwindOutput,
};
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW},
    database::Database,
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_primitives::{keccak256, Address, HashedStorageEntry, TransitionId};
use std::fmt::Debug;
use tracing::*;

const STORAGE_HASHING: StageId = StageId("StorageHashing");

/// The storage hashing stage iterates over existing account
/// storages, and with them populates the
/// [`HashedStorage`][reth_interfaces::db::tables::HashedStorage] table.
#[derive(Debug)]
pub struct StorageHashingStage {
    /// The size of inserted items after which the control
    /// flow will be returned to the pipeline for commit
    pub commit_threshold: u64,
    /// The threshold for switching from incremental hashing
    /// of changes to whole storage hashing
    pub clean_threshold: u64,
}

#[async_trait::async_trait]
impl<DB: Database> Stage<DB> for StorageHashingStage {
    /// Return the id of the stage
    fn id(&self) -> StageId {
        STORAGE_HASHING
    }

    /// Turn each StorageEntry into a
    /// [`HashedStorageEntry`][reth_primitives::storage::HashedStorageEntry],
    /// by hashing it's key, and save them into the
    /// [`HashedStorage`][reth_interfaces::db::tables::HashedStorage] table,
    /// under a hashed account address.
    /// If the range of transitions is lower than clean_threshold, updates only
    /// changed entries.
    async fn execute(
        &mut self,
        tx: &mut Transaction<'_, DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        let stage_progress = input.stage_progress.unwrap_or_default();
        let previous_stage_progress = input.previous_stage_progress();
        let max_block_num = previous_stage_progress.min(stage_progress + self.commit_threshold);

        if max_block_num <= stage_progress {
            info!(target: "sync::stages::storage_hashing", target = max_block_num, stage_progress, "Target block already reached");
            return Ok(ExecOutput { stage_progress, done: true })
        }

        let start_transition: TransitionId = tx.get_block_transition_by_num(stage_progress)? + 1;
        let end_transition: TransitionId = tx.get_block_transition_by_num(max_block_num)?;

        // No transitions to walk over
        if start_transition > end_transition {
            info!(target: "sync::stages::storage_hashing", start_transition, end_transition, "Target transition already reached");
            return Ok(ExecOutput { stage_progress: max_block_num, done: true })
        } else if end_transition - start_transition > self.clean_threshold || stage_progress == 0 {
            // There are too many transitions, so we rehash all storage entries
            tx.clear::<tables::HashedStorage>()?;

            let mut storage_cursor = tx.cursor_dup::<tables::PlainStorageState>()?;
            let mut hashed_storage_cursor = tx.cursor_dup_mut::<tables::HashedStorage>()?;

            let mut walker = storage_cursor.walk(Address::zero())?;

            while let Some((address, entry)) = walker.next().transpose()? {
                let hashed_entry = HashedStorageEntry { key: keccak256(entry.key), ..entry };
                // TODO: we should order this data before insertion
                hashed_storage_cursor.append_dup(keccak256(address), hashed_entry)?;
            }
            return Ok(ExecOutput { stage_progress: previous_stage_progress, done: true })
        }

        // Acquire the Storage cursor
        let mut storage_cursor = tx.cursor_dup::<tables::PlainStorageState>()?;
        // Acquire the changeset cursor
        let mut storage_changeset_cursor = tx.cursor::<tables::StorageChangeSet>()?;
        // Acquire the cursor for inserting elements
        let mut hashed_storage_cursor = tx.cursor_dup_mut::<tables::HashedStorage>()?;

        // Walk the transactions from start to end index (inclusive)
        let mut walker =
            storage_changeset_cursor.walk(start_transition.into())?.take_while(|res| {
                res.as_ref()
                    .map(|(k, _)| (*k).transition_id() <= end_transition)
                    .unwrap_or_default()
            });

        // Iterate over transactions in chunks
        info!(target: "sync::stages::storage_hashing", start_transition, end_transition, "Hashing storage");
        while let Some((tid_address, entry)) = walker.next().transpose()? {
            let new_address = keccak256(tid_address.address());
            let new_key = keccak256(entry.key);

            // Delete current hashed entry
            let _ = hashed_storage_cursor.seek_by_key_subkey(new_address, new_key);
            let _ = hashed_storage_cursor.delete_current();

            if let Some(current_se) =
                storage_cursor.seek_by_key_subkey(tid_address.address(), entry.key)?
            {
                // Create and append new entry with a hashed key
                let hashed_se = HashedStorageEntry { key: new_key, ..current_se };
                // TODO: we should order this data before insertion
                hashed_storage_cursor.append_dup(new_address, hashed_se)?;
            }
        }

        let done = max_block_num >= previous_stage_progress;
        info!(target: "sync::stages::storage_hashing", stage_progress = max_block_num, done, "Sync iteration finished");
        Ok(ExecOutput { stage_progress: max_block_num, done })
    }

    /// Unwind the stage.
    async fn unwind(
        &mut self,
        _tx: &mut Transaction<'_, DB>,
        _input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        unimplemented!();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{
        stage_test_suite_ext, ExecuteStageTestRunner, StageTestRunner, TestRunnerError,
        TestTransaction, UnwindStageTestRunner, PREV_STAGE_ID,
    };
    use assert_matches::assert_matches;
    use reth_db::models::StoredBlockBody;
    use reth_interfaces::test_utils::generators::random_block_range;
    use reth_primitives::{
        SealedBlock, StorageEntry, Transaction, TransactionKind, TxLegacy, H256, U256,
    };

    stage_test_suite_ext!(StorageHashingTestRunner);

    /// Execute a block range with a single account and storage
    #[tokio::test]
    async fn execute_single_account() {
        let (previous_stage, stage_progress) = (500, 100);

        // Set up the runner
        let mut runner = StorageHashingTestRunner::default();
        // set low threshold so we hash the whole storage
        runner.set_clean_threshold(1);
        let input = ExecInput {
            previous_stage: Some((PREV_STAGE_ID, previous_stage)),
            stage_progress: Some(stage_progress),
        };

        runner.seed_execution(input).expect("failed to seed execution");

        let rx = runner.execute(input);

        // Assert the successful result
        let result = rx.await.unwrap();
        assert_matches!(
            result,
            Ok(ExecOutput { done, stage_progress })
                if done && stage_progress == previous_stage
        );

        // Validate the stage execution
        assert!(runner.validate_execution(input, result.ok()).is_ok(), "execution validation");
    }

    struct StorageHashingTestRunner {
        tx: TestTransaction,
        commit_threshold: u64,
        clean_threshold: u64,
    }

    impl Default for StorageHashingTestRunner {
        fn default() -> Self {
            Self { tx: TestTransaction::default(), commit_threshold: 1000, clean_threshold: 1000 }
        }
    }

    impl StageTestRunner for StorageHashingTestRunner {
        type S = StorageHashingStage;

        fn tx(&self) -> &TestTransaction {
            &self.tx
        }

        fn stage(&self) -> Self::S {
            Self::S {
                commit_threshold: self.commit_threshold,
                clean_threshold: self.clean_threshold,
            }
        }
    }

    #[async_trait::async_trait]
    impl ExecuteStageTestRunner for StorageHashingTestRunner {
        type Seed = Vec<SealedBlock>;

        fn seed_execution(&mut self, input: ExecInput) -> Result<Self::Seed, TestRunnerError> {
            let stage_progress = input.stage_progress.unwrap_or_default();
            let end = input.previous_stage_progress() + 1;

            let blocks = random_block_range(stage_progress..end, H256::zero(), 0..2);

            self.tx.insert_headers(blocks.iter().map(|block| &block.header))?;

            let mut iter = blocks.iter();
            let (mut transition_id, mut tx_id) = (0, 0);

            while let Some(progress) = iter.next() {
                // Insert last progress data
                self.tx.commit(|tx| {
                    let key = (progress.number, progress.hash()).into();

                    let body = StoredBlockBody {
                        start_tx_id: tx_id,
                        tx_count: progress.body.len() as u64,
                    };

                    progress.body.iter().try_for_each(|transaction| {
                        tx.put::<tables::TxHashNumber>(transaction.hash(), tx_id)?;
                        tx.put::<tables::Transactions>(tx_id, transaction.clone())?;
                        tx.put::<tables::TxTransitionIndex>(tx_id, tx_id)?;
                        tx_id += 1;
                        let (to, value) = match transaction.transaction {
                            Transaction::Legacy(TxLegacy {
                                to: TransactionKind::Call(to),
                                value,
                                ..
                            }) => (to, value),
                            _ => unreachable!(),
                        };
                        let entry =
                            StorageEntry { key: keccak256("transfers"), value: U256::from(value) };
                        tx.cursor_dup_mut::<tables::PlainStorageState>()?.upsert(to, entry)
                    })?;

                    // Randomize rewards
                    let has_reward: bool = rand::random();
                    transition_id += progress.body.len().saturating_sub(1) as u64 +
                        if has_reward { 1 } else { 0 };

                    tx.put::<tables::BlockTransitionIndex>(key, transition_id)?;
                    tx.put::<tables::BlockBodies>(key, body)?;
                    Ok(())
                })?;
            }

            Ok(blocks)
        }

        fn validate_execution(
            &self,
            input: ExecInput,
            output: Option<ExecOutput>,
        ) -> Result<(), TestRunnerError> {
            if let Some(output) = output {
                let start_block = input.stage_progress.unwrap_or_default() + 1;
                let end_block = output.stage_progress;
                if start_block > end_block {
                    return Ok(())
                }
            }
            self.check_hashed_storage()
        }
    }

    impl UnwindStageTestRunner for StorageHashingTestRunner {
        fn validate_unwind(&self, _input: UnwindInput) -> Result<(), TestRunnerError> {
            self.check_hashed_storage()
        }
    }

    impl StorageHashingTestRunner {
        // fn set_commit_threshold(&mut self, threshold: u64) {
        //     self.commit_threshold = threshold;
        // }

        fn set_clean_threshold(&mut self, threshold: u64) {
            self.clean_threshold = threshold;
        }

        fn check_hashed_storage(&self) -> Result<(), TestRunnerError> {
            self.tx
                .query(|tx| {
                    let mut storage_cursor = tx.cursor_dup::<tables::PlainStorageState>()?;
                    let mut hashed_storage_cursor = tx.cursor_dup::<tables::HashedStorage>()?;

                    let mut expected = 0;

                    while let Some((address, entry)) = storage_cursor.next()? {
                        let key = keccak256(entry.key);
                        assert_eq!(
                            hashed_storage_cursor.seek_by_key_subkey(keccak256(address), key)?,
                            Some(HashedStorageEntry { key, ..entry })
                        );
                        expected += 1;
                    }
                    let count =
                        tx.cursor_dup::<tables::HashedStorage>()?.walk([0; 32].into())?.count();

                    assert_eq!(count, expected);
                    Ok(())
                })
                .map_err(|e| e.into())
        }
    }
}
