use crate::{
    db::Transaction, ExecInput, ExecOutput, Stage, StageError, StageId, UnwindInput, UnwindOutput,
};
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW},
    database::Database,
    models::AccountBeforeTx,
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_primitives::{keccak256, Address};
use tracing::*;

const ACCOUNT_HASHING: StageId = StageId("AccountHashing");

/// The account hashing stage iterates over account states,
/// hashes their addresses and inserts entries to the
/// [`HashedAccount`][reth_interfaces::db::tables::HashedAccount] table.
#[derive(Debug)]
pub struct AccountHashingStage {
    /// The size of inserted items after which the control
    /// flow will be returned to the pipeline for commit
    pub commit_threshold: u64,
    /// The threshold for switching from incremental hashing
    /// of changes to whole account hashing
    pub clean_threshold: u64,
}

#[async_trait::async_trait]
impl<DB: Database> Stage<DB> for AccountHashingStage {
    /// Return the id of the stage
    fn id(&self) -> StageId {
        ACCOUNT_HASHING
    }

    async fn execute(
        &mut self,
        tx: &mut Transaction<'_, DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        let previous_stage_progress = input.previous_stage_progress();
        let stage_progress = input.stage_progress.unwrap_or_default();
        let max_block_num = previous_stage_progress.min(stage_progress + self.commit_threshold);

        // TODO: use macro here
        if max_block_num <= stage_progress {
            info!(target: "sync::stages::account_hashing", target = max_block_num, stage_progress, "Target block already reached");
            return Ok(ExecOutput { stage_progress, done: true })
        }

        // We get the last transition from block `stage_progress` and add one to get the transition
        // from the next block.
        let start_transition = tx.get_block_transition_by_num(stage_progress)? + 1;
        let end_transition = tx.get_block_transition_by_num(max_block_num)?;

        if start_transition > end_transition {
            // No transitions to walk over
            info!(target: "sync::stages::account_hashing", start_transition, end_transition, "Target transition already reached");
            return Ok(ExecOutput { stage_progress: max_block_num, done: true })
        } else if end_transition - start_transition > self.clean_threshold || stage_progress == 0 {
            // There are too many transitions, so we rehash all accounts
            tx.clear::<tables::HashedAccount>()?;

            let mut account_cursor = tx.cursor::<tables::PlainAccountState>()?;
            let mut hashed_account_cursor = tx.cursor_mut::<tables::HashedAccount>()?;

            let mut walker = account_cursor.walk(Address::zero())?;

            while let Some((address, account)) = walker.next().transpose()? {
                let hashed_address = keccak256(address);
                hashed_account_cursor.append(hashed_address, account)?;
            }

            return Ok(ExecOutput { stage_progress: previous_stage_progress, done: true })
        }

        // Acquire the PlainAccount cursor
        let mut acc_cursor = tx.cursor::<tables::PlainAccountState>()?;
        // Acquire the AccountChangeSet cursor
        let mut acc_changeset_cursor = tx.cursor::<tables::AccountChangeSet>()?;
        // Acquire the cursor for inserting elements
        let mut hashed_acc_cursor = tx.cursor_mut::<tables::HashedAccount>()?;

        let mut walker = acc_changeset_cursor.walk(start_transition)?.take_while(|ch_entry| {
            ch_entry.as_ref().map(|(tid, _)| tid <= &end_transition).unwrap_or_default()
        });

        // Iterate over transactions in chunks
        info!(target: "sync::stages::account_hashing", start_transition, end_transition, "Hashing accounts");
        while let Some((_, AccountBeforeTx { address, .. })) = walker.next().transpose()? {
            // Query updated account
            // If it was not deleted, upsert hashed account table
            if let Some((_, acc)) = acc_cursor.seek_exact(address)? {
                hashed_acc_cursor.upsert(keccak256(address), acc)?;
            // If account was deleted, delete entry from the hashed accounts table
            } else if hashed_acc_cursor.seek_exact(keccak256(address))?.is_some() {
                hashed_acc_cursor.delete_current()?;
            }
        }

        let done = max_block_num >= previous_stage_progress;
        info!(target: "sync::stages::account_hashing", stage_progress = max_block_num, done, "Sync iteration finished");

        Ok(ExecOutput { stage_progress: max_block_num, done })
    }

    /// Unwind the stage.
    async fn unwind(
        &mut self,
        tx: &mut Transaction<'_, DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        // Get the last transition of the `unwind_to` block to know when unwinding should stop
        let end_transition = tx.get_block_transition_by_num(input.unwind_to)?;

        let mut hashed_acc_cursor = tx.cursor_mut::<tables::HashedAccount>()?;
        let mut acc_changeset_cursor = tx.cursor::<tables::AccountChangeSet>()?;

        let mut entry = acc_changeset_cursor.last()?;
        while let Some((tid, ref acc_before_tx)) = entry {
            if tid <= end_transition {
                break
            }
            let hashed_addr = keccak256(acc_before_tx.address);
            if let Some(acc) = acc_before_tx.info {
                hashed_acc_cursor.upsert(hashed_addr, acc)?;
            } else if hashed_acc_cursor.seek_exact(hashed_addr)?.is_some() {
                hashed_acc_cursor.delete_current()?;
            }

            entry = acc_changeset_cursor.prev()?;
        }

        Ok(UnwindOutput { stage_progress: input.unwind_to })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{
        ExecuteStageTestRunner, StageTestRunner, TestRunnerError, TestTransaction,
        UnwindStageTestRunner, PREV_STAGE_ID,
    };
    use assert_matches::assert_matches;
    use reth_db::models::StoredBlockBody;
    use reth_interfaces::test_utils::generators::random_block_range;
    use reth_primitives::{
        SealedBlock, StorageEntry, Transaction, TransactionKind, TxLegacy, H256, U256,
    };
    use test_utils::*;

    /// Execute a block range with a single account and storage
    #[tokio::test]
    async fn execute_single_account() {
        let (previous_stage, stage_progress) = (500, 100);

        // Set up the runner
        let mut runner = AccountHashingTestRunner::default();
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

    mod test_utils {
        use crate::{
            test_utils::{
                TestTransaction, StageTestRunner,
            },
            ExecInput, ExecOutput, UnwindInput, stages::account_hashes::AccountHashingStage,
        };
        use assert_matches::assert_matches;
        use reth_db::{
            cursor::DbCursorRO,
            models::{BlockNumHash, StoredBlockBody, StoredBlockOmmers},
            tables,
            transaction::{DbTx, DbTxMut},
        };

        pub(crate) struct AccountHashingTestRunner {
            pub(crate) tx: TestTransaction,
            commit_threshold: u64,
            clean_threshold: u64,
        }

        impl Default for AccountHashingTestRunner {
            fn default() -> Self {
                Self { tx: TestTransaction::default(), commit_threshold: 1000, clean_threshold: 1000 }
            }
        }

        impl AccountHashingTestRunner {
            fn set_clean_threshold(&mut self, threshold: u64) {
                self.clean_threshold = threshold;
            } 
        }

        impl StageTestRunner for AccountHashingTestRunner {
            type S = AccountHashingStage;
    
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
        
    }

    #[async_trait::async_trait]
    impl ExecuteStageTestRunner for AccountHashingTestRunner {
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
