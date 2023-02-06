use crate::{
    db::Transaction, ExecInput, ExecOutput, Stage, StageError, StageId, UnwindInput, UnwindOutput,
};
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO},
    database::Database,
    models::TransitionIdAddress,
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_primitives::{keccak256, Address, StorageEntry, H160, H256, U256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Debug,
};
use tracing::*;

/// The [`StageId`] of the storage hashing stage.
pub const STORAGE_HASHING: StageId = StageId("StorageHashing");

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
        let from_transition = tx.get_block_transition(stage_progress)?;
        let to_transition = tx.get_block_transition(previous_stage_progress)?;

        // if there are more blocks then threshold it is faster to go over Plain state and hash all
        // account otherwise take changesets aggregate the sets and apply hashing to
        // AccountHashing table. Also, if we start from genesis, we need to hash from scratch, as
        // genesis accounts are not in changeset, along with their storages.
        if to_transition - from_transition > self.clean_threshold || stage_progress == 0 {
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
                            res.map(|(address, slot)| {
                                // both account address and storage slot key are hashed for merkle
                                // tree.
                                ((keccak256(address), keccak256(slot.key)), slot.value)
                            })
                        })
                        .collect::<Result<BTreeMap<_, _>, _>>()?;

                    // next key of iterator
                    let next_key = storage.next()?;

                    // iterate and put presorted hashed slots
                    hashed_batch.into_iter().try_for_each(|((addr, key), value)| {
                        tx.put::<tables::HashedStorage>(addr, StorageEntry { key, value })
                    })?;
                    next_key.map(|(key, _)| key)
                };
                tx.commit()?;

                first_key = match next_key {
                    Some(key) => key,
                    None => break,
                };
            }
        } else {
            let mut plain_storage = tx.cursor_dup_read::<tables::PlainStorageState>()?;
            let mut hashed_storage = tx.cursor_dup_write::<tables::HashedStorage>()?;

            // Aggregate all transition changesets and and make list of storages that have been
            // changed.
            tx.cursor_read::<tables::StorageChangeSet>()?
                .walk_range(
                    (from_transition, Address::zero()).into()..
                        (to_transition, Address::zero()).into(),
                )?
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                // fold all storages and save its old state so we can remove it from HashedStorage
                // it is needed as it is dup table.
                .fold(
                    BTreeMap::new(),
                    |mut accounts: BTreeMap<Address, BTreeSet<H256>>,
                     (TransitionIdAddress((_, address)), storage_entry)| {
                        accounts.entry(address).or_default().insert(storage_entry.key);
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
                        .map(|key| {
                            plain_storage
                                .seek_by_key_subkey(address, key)
                                .map(|ret| (keccak256(key), ret.map(|e| e.value)))
                        })
                        .collect::<Result<BTreeMap<_, _>, _>>()
                        .map(|storage| (keccak256(address), storage))
                })
                .collect::<Result<BTreeMap<_, _>, _>>()?
                .into_iter()
                // Hash the address and key and apply them to HashedStorage (if Storage is None
                // just remove it);
                .try_for_each(|(address, storage)| {
                    storage.into_iter().try_for_each(|(key, val)| -> Result<(), StageError> {
                        if hashed_storage
                            .seek_by_key_subkey(address, key)?
                            .filter(|entry| entry.key == key)
                            .is_some()
                        {
                            hashed_storage.delete_current()?;
                        }

                        if let Some(value) = val {
                            hashed_storage.upsert(address, StorageEntry { key, value })?;
                        }
                        Ok(())
                    })
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
        let from_transition_rev = tx.get_block_transition(input.unwind_to)?;
        let to_transition_rev = tx.get_block_transition(input.stage_progress)?;

        let mut hashed_storage = tx.cursor_dup_write::<tables::HashedStorage>()?;

        // Aggregate all transition changesets and make list of accounts that have been changed.
        tx.cursor_read::<tables::StorageChangeSet>()?
            .walk_range(
                (from_transition_rev, Address::zero()).into()..
                    (to_transition_rev, Address::zero()).into(),
            )?
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
            // Apply values to HashedStorage (if Value is zero just remove it);
            .try_for_each(|((address, key), value)| -> Result<(), StageError> {
                if hashed_storage
                    .seek_by_key_subkey(address, key)?
                    .filter(|entry| entry.key == key)
                    .is_some()
                {
                    hashed_storage.delete_current()?;
                }

                if value != U256::ZERO {
                    hashed_storage.upsert(address, StorageEntry { key, value })?;
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
    use reth_interfaces::test_utils::generators::{
        random_block_range, random_contract_account_range,
    };
    use reth_primitives::{SealedBlock, StorageEntry, H256, U256};

    stage_test_suite_ext!(StorageHashingTestRunner, storage_hashing);

    /// Execute with low clean threshold so as to hash whole storage
    #[tokio::test]
    async fn execute_clean_storage_hashing() {
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

            let n_accounts = 31;
            let mut accounts = random_contract_account_range(&mut (0..n_accounts));

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

                        let (addr, _) = accounts
                            .get_mut(rand::random::<usize>() % n_accounts as usize)
                            .unwrap();

                        let new_entry = StorageEntry {
                            key: keccak256([rand::random::<u8>()]),
                            value: U256::from(rand::random::<u8>() % 30 + 1),
                        };
                        self.insert_storage_entry(
                            tx,
                            (transition_id, *addr).into(),
                            new_entry,
                            progress.header.number == stage_progress,
                        )?;
                        tx_id += 1;
                        transition_id += 1;
                        Ok(())
                    })?;

                    // Randomize rewards
                    let has_reward: bool = rand::random();
                    if has_reward {
                        self.insert_storage_entry(
                            tx,
                            (transition_id, Address::random()).into(),
                            StorageEntry {
                                key: keccak256("mining"),
                                value: U256::from(rand::random::<u32>()),
                            },
                            progress.header.number == stage_progress,
                        )?;
                        transition_id += 1;
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
                    let count =
                        tx.cursor_dup_read::<tables::HashedStorage>()?.walk(H256::zero())?.count();

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
            let prev_entry =
                match storage_cursor.seek_by_key_subkey(tid_address.address(), entry.key)? {
                    Some(e) if e.key == entry.key => {
                        tx.delete::<tables::PlainStorageState>(tid_address.address(), Some(e))
                            .expect("failed to delete entry");
                        e
                    }
                    _ => StorageEntry { key: entry.key, value: U256::from(0) },
                };
            tx.put::<tables::PlainStorageState>(tid_address.address(), entry)?;

            if hash {
                let hashed_address = keccak256(tid_address.address());
                let hashed_entry = StorageEntry { key: keccak256(entry.key), value: entry.value };

                if let Some(e) = tx
                    .cursor_dup_write::<tables::HashedStorage>()?
                    .seek_by_key_subkey(hashed_address, hashed_entry.key)?
                    .filter(|e| e.key == hashed_entry.key)
                {
                    tx.delete::<tables::HashedStorage>(hashed_address, Some(e))
                        .expect("failed to delete entry");
                }

                tx.put::<tables::HashedStorage>(hashed_address, hashed_entry)?;
            }

            tx.put::<tables::StorageChangeSet>(tid_address, prev_entry)?;
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
                    if tid_address.transition_id() < target_transition {
                        break
                    }

                    if storage_cursor
                        .seek_by_key_subkey(tid_address.address(), entry.key)?
                        .filter(|e| e.key == entry.key)
                        .is_some()
                    {
                        storage_cursor.delete_current()?;
                    }

                    if entry.value != U256::ZERO {
                        storage_cursor.upsert(tid_address.address(), entry)?;
                    }
                }
                Ok(())
            })?;
            Ok(())
        }
    }
}
