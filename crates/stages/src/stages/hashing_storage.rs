use crate::{
    db::Transaction, ExecInput, ExecOutput, Stage, StageError, StageId, UnwindInput, UnwindOutput,
};
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW},
    database::Database,
    models::TransitionIdAddress,
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_primitives::{keccak256, Address, StorageEntry, H160, H256, U256};
use std::{collections::BTreeMap, fmt::Debug};
use tracing::*;

/// The [`StageId`] of the storage hashing stage.
pub const STORAGE_HASHING: StageId = StageId("StorageHashingStage");

/// Storage hashing stage hashes plain storage.
/// This is preparation before generating intermediate hashes and calculating Merkle tree root.
#[derive(Debug)]
pub struct StorageHashingStage {
    /// The threshold (in number of state transitions) for switching between incremental
    /// hashing and full storage hashing.
    pub clean_threshold: u64,
    /// The maximum number of blocks to process before committing.
    pub commit_threshold: u64,
}

impl Default for StorageHashingStage {
    fn default() -> Self {
        Self { clean_threshold: 500_000, commit_threshold: 100_000 }
    }
}

#[async_trait::async_trait]
impl<DB: Database> Stage<DB> for StorageHashingStage {
    /// Return the id of the stage
    fn id(&self) -> StageId {
        STORAGE_HASHING
    }

    /// Execute the stage.
    async fn execute(
        &mut self,
        tx: &mut Transaction<'_, DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        let stage_progress = input.stage_progress.unwrap_or_default();
        let previous_stage_progress = input.previous_stage_progress();

        // read storage changeset, merge it into one changeset and calculate storage hashes.
        let from_transition = tx.get_block_transition(stage_progress)? + 1;
        let to_transition = tx.get_block_transition(previous_stage_progress)? + 1;

        // if there are more blocks then threshold it is faster to go over Plain state and hash all
        // account otherwise take changesets aggregate the sets and apply hashing to
        // AccountHashing table
        if to_transition - from_transition > self.clean_threshold {
            tx.clear::<tables::HashedStorage>()?;
            tx.commit()?;

            let mut first_key = H160::zero();
            loop {
                let next_key = {
                    let mut storage = tx.cursor_dup_read::<tables::PlainStorageState>()?;

                    let hashed_batch = storage
                        .walk(first_key)?
                        .take(self.commit_threshold as usize)
                        .map(|res| {
                            res.map(|(address, mut slot)| {
                                // both account address and storage slot key are hashed for merkle
                                // tree.
                                slot.key = keccak256(slot.key);
                                (keccak256(address), slot)
                            })
                        })
                        .collect::<Result<BTreeMap<_, _>, _>>()?;

                    // next key of iterator
                    let next_key = storage.next()?;

                    let mut hashes = tx.cursor_write::<tables::HashedStorage>()?;
                    // iterate and append presorted hashed slots
                    hashed_batch.into_iter().try_for_each(|(k, v)| hashes.append(k, v))?;
                    next_key
                };
                tx.commit()?;
                if let Some((next_key, _)) = next_key {
                    first_key = next_key;
                    continue
                }
                break
            }
        } else {
            let mut plain_storage = tx.cursor_dup_read::<tables::PlainStorageState>()?;

            // Aggregate all transition changesets and and make list of storages that have been
            // changed.
            tx.cursor_read::<tables::StorageChangeSet>()?
                .walk((from_transition, H160::zero()).into())?
                .take_while(|res| {
                    res.as_ref().map(|(k, _)| k.0 .0 < to_transition).unwrap_or_default()
                })
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                // fold all storages and save its old state so we can remove it from HashedStorage
                // it is needed as it is dup table.
                .fold(
                    BTreeMap::new(),
                    |mut accounts: BTreeMap<Address, BTreeMap<H256, U256>>,
                     (TransitionIdAddress((_, address)), storage_entry)| {
                        accounts
                            .entry(address)
                            .or_default()
                            .insert(storage_entry.key, storage_entry.value);
                        accounts
                    },
                )
                .into_iter()
                // iterate over plain state and get newest storage value.
                // Assumption we are okay with is that plain state represent
                // `previous_stage_progress` state.
                .map(|(address, storage)| {
                    storage
                        .into_iter()
                        .map(|(key, val)| {
                            plain_storage
                                .seek_by_key_subkey(address, key)
                                .map(|ret| (key, (val, ret.map(|e| e.value))))
                        })
                        .collect::<Result<BTreeMap<_, _>, _>>()
                        .map(|storage| (address, storage))
                })
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                // Hash the address and key and apply them to HashedStorage (if Storage is None
                // just remove it);
                .try_for_each(|(address, storage)| {
                    let hashed_address = keccak256(address);
                    storage.into_iter().try_for_each(
                        |(key, (old_val, new_val))| -> Result<(), StageError> {
                            let key = keccak256(key);
                            tx.delete::<tables::HashedStorage>(
                                hashed_address,
                                Some(StorageEntry { key, value: old_val }),
                            )?;
                            if let Some(value) = new_val {
                                let val = StorageEntry { key, value };
                                tx.put::<tables::HashedStorage>(hashed_address, val)?
                            }
                            Ok(())
                        },
                    )
                })?;
        }

        info!(target: "sync::stages::hashing_storage", "Stage finished");
        Ok(ExecOutput { stage_progress: input.previous_stage_progress(), done: true })
    }

    /// Unwind the stage.
    async fn unwind(
        &mut self,
        tx: &mut Transaction<'_, DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        let from_transition_rev = tx.get_block_transition(input.unwind_to)? + 1;
        let to_transition_rev = tx.get_block_transition(input.stage_progress)? + 1;

        let mut hashed_storage = tx.cursor_dup_write::<tables::HashedStorage>()?;

        // Aggregate all transition changesets and make list of accounts that have been changed.
        tx.cursor_read::<tables::StorageChangeSet>()?
            .walk((from_transition_rev, H160::zero()).into())?
            .take_while(|res| {
                res.as_ref()
                    .map(|(TransitionIdAddress((k, _)), _)| *k < to_transition_rev)
                    .unwrap_or_default()
            })
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .rev()
            // fold all account to get the old balance/nonces and account that needs to be removed
            .fold(
                BTreeMap::new(),
                |mut accounts: BTreeMap<(Address, H256), U256>,
                 (TransitionIdAddress((_, address)), storage_entry)| {
                    accounts.insert((address, storage_entry.key), storage_entry.value);
                    accounts
                },
            )
            .into_iter()
            // hash addresses and collect it inside sorted BTreeMap.
            // We are doing keccak only once per address.
            .map(|((address, key), value)| ((keccak256(address), keccak256(key)), value))
            .collect::<BTreeMap<_, _>>()
            .into_iter()
            // Apply values to HashedStorage (if Value is zero remove it);
            .try_for_each(|((address, key), value)| -> Result<(), StageError> {
                hashed_storage.seek_by_key_subkey(address, key)?;
                hashed_storage.delete_current()?;

                if value != U256::ZERO {
                    hashed_storage.append_dup(address, StorageEntry { key, value })?;
                }
                Ok(())
            })?;

        Ok(UnwindOutput { stage_progress: input.unwind_to })
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
    use reth_db::{
        cursor::DbCursorRW,
        mdbx::{tx::Tx, WriteMap, RW},
        models::{BlockNumHash, StoredBlockBody, TransitionIdAddress},
    };
    use reth_interfaces::test_utils::generators::random_block_range;
    use reth_primitives::{
        SealedBlock, StorageEntry, Transaction, TransactionKind, TxLegacy, H256, U256,
    };

    stage_test_suite_ext!(StorageHashingTestRunner, storage_hashing);

    /// Execute with low clean threshold so as to hash whole storage
    #[tokio::test]
    async fn execute_clean() {
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

            let blocks = random_block_range(stage_progress..end, H256::zero(), 0..3);

            self.tx.insert_headers(blocks.iter().map(|block| &block.header))?;

            let iter = blocks.iter();
            let (mut transition_id, mut tx_id) = (0, 0);

            for progress in iter {
                // Insert last progress data
                self.tx.commit(|tx| {
                    let key: BlockNumHash = (progress.number, progress.hash()).into();

                    let body = StoredBlockBody {
                        start_tx_id: tx_id,
                        tx_count: progress.body.len() as u64,
                    };

                    progress.body.iter().try_for_each(|transaction| {
                        tx.put::<tables::TxHashNumber>(transaction.hash(), tx_id)?;
                        tx.put::<tables::Transactions>(tx_id, transaction.clone())?;
                        tx.put::<tables::TxTransitionIndex>(tx_id, transition_id)?;
                        tx_id += 1;
                        transition_id += 1;
                        let (to, value) = match transaction.transaction {
                            Transaction::Legacy(TxLegacy {
                                to: TransactionKind::Call(to),
                                value,
                                ..
                            }) => (to, value),
                            _ => unreachable!(),
                        };
                        let new_entry =
                            StorageEntry { key: keccak256("transfers"), value: U256::from(value) };
                        self.insert_storage_entry(
                            tx,
                            (transition_id, to).into(),
                            new_entry,
                            progress.header.number == stage_progress,
                        )
                    })?;

                    // Randomize rewards
                    let has_reward: bool = rand::random();
                    if has_reward {
                        transition_id += 1;
                        self.insert_storage_entry(
                            tx,
                            (transition_id, Address::random()).into(),
                            StorageEntry {
                                key: keccak256("mining"),
                                value: U256::from(rand::random::<u32>()),
                            },
                            progress.header.number == stage_progress,
                        )?;
                    }

                    tx.put::<tables::BlockTransitionIndex>(key.number(), transition_id)?;
                    tx.put::<tables::BlockBodies>(key, body)
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
        fn validate_unwind(&self, input: UnwindInput) -> Result<(), TestRunnerError> {
            self.unwind_storage(input)?;
            self.check_hashed_storage()
        }
    }

    impl StorageHashingTestRunner {
        fn set_clean_threshold(&mut self, threshold: u64) {
            self.clean_threshold = threshold;
        }

        fn check_hashed_storage(&self) -> Result<(), TestRunnerError> {
            self.tx
                .query(|tx| {
                    let mut storage_cursor = tx.cursor_dup_read::<tables::PlainStorageState>()?;
                    let mut hashed_storage_cursor =
                        tx.cursor_dup_read::<tables::HashedStorage>()?;

                    let mut expected = 0;

                    while let Some((address, entry)) = storage_cursor.next()? {
                        let key = keccak256(entry.key);
                        let got =
                            hashed_storage_cursor.seek_by_key_subkey(keccak256(address), key)?;
                        assert_eq!(
                            got,
                            Some(StorageEntry { key, ..entry }),
                            "{expected}: {address:?}"
                        );
                        expected += 1;
                    }
                    let count = tx
                        .cursor_dup_read::<tables::HashedStorage>()?
                        .walk([0; 32].into())?
                        .count();

                    assert_eq!(count, expected);
                    Ok(())
                })
                .map_err(|e| e.into())
        }

        fn insert_storage_entry(
            &self,
            tx: &Tx<'_, RW, WriteMap>,
            tid_address: TransitionIdAddress,
            entry: StorageEntry,
            hash: bool,
        ) -> Result<(), reth_db::Error> {
            let mut storage_cursor = tx.cursor_dup_write::<tables::PlainStorageState>()?;
            let prev_entry = storage_cursor
                .seek_by_key_subkey(tid_address.address(), entry.key)?
                .map(|e| {
                    storage_cursor.delete_current().expect("failed to delete entry");
                    e
                })
                .unwrap_or(StorageEntry { key: entry.key, value: U256::from(0) });
            if hash {
                tx.cursor_dup_write::<tables::HashedStorage>()?.append_dup(
                    keccak256(tid_address.address()),
                    StorageEntry { key: keccak256(entry.key), value: entry.value },
                )?;
            }
            storage_cursor.append_dup(tid_address.address(), entry)?;

            tx.cursor_dup_write::<tables::StorageChangeSet>()?
                .append_dup(tid_address, prev_entry)?;
            Ok(())
        }

        fn unwind_storage(&self, input: UnwindInput) -> Result<(), TestRunnerError> {
            tracing::debug!("unwinding storage...");
            let target_transition = self
                .tx
                .inner()
                .get_block_transition(input.unwind_to)
                .map_err(|e| TestRunnerError::Internal(Box::new(e)))?;
            self.tx.commit(|tx| {
                let mut storage_cursor = tx.cursor_dup_write::<tables::PlainStorageState>()?;
                let mut changeset_cursor = tx.cursor_dup_read::<tables::StorageChangeSet>()?;

                let mut rev_changeset_walker = changeset_cursor.walk_back(None)?;

                while let Some((tid_address, entry)) = rev_changeset_walker.next().transpose()? {
                    if tid_address.transition_id() <= target_transition {
                        break
                    }

                    storage_cursor.seek_by_key_subkey(tid_address.address(), entry.key)?;
                    storage_cursor.delete_current()?;

                    if entry.value != U256::ZERO {
                        storage_cursor.append_dup(tid_address.address(), entry)?;
                    }
                }
                Ok(())
            })?;
            Ok(())
        }
    }
}
