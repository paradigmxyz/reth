use crate::{ExecInput, ExecOutput, Stage, StageError, UnwindInput, UnwindOutput};
use num_traits::Zero;
use reth_db::{
    cursor::DbDupCursorRO,
    database::Database,
    models::BlockNumberAddress,
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_interfaces::db::DatabaseError;
use reth_primitives::{
    keccak256,
    stage::{
        CheckpointBlockRange, EntitiesCheckpoint, StageCheckpoint, StageId,
        StorageHashingCheckpoint,
    },
    StorageEntry,
};
use reth_provider::Transaction;
use std::{collections::BTreeMap, fmt::Debug, ops::Deref};
use tracing::*;

/// Storage hashing stage hashes plain storage.
/// This is preparation before generating intermediate hashes and calculating Merkle tree root.
#[derive(Debug)]
pub struct StorageHashingStage {
    /// The threshold (in number of blocks) for switching between incremental
    /// hashing and full storage hashing.
    pub clean_threshold: u64,
    /// The maximum number of slots to process before committing.
    pub commit_threshold: u64,
}

impl StorageHashingStage {
    /// Create new instance of [StorageHashingStage].
    pub fn new(clean_threshold: u64, commit_threshold: u64) -> Self {
        Self { clean_threshold, commit_threshold }
    }
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
        StageId::StorageHashing
    }

    /// Execute the stage.
    async fn execute(
        &mut self,
        tx: &mut Transaction<'_, DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        let range = input.next_block_range();
        if range.is_empty() {
            return Ok(ExecOutput::done(StageCheckpoint::new(*range.end())))
        }
        let (from_block, to_block) = range.into_inner();

        // if there are more blocks then threshold it is faster to go over Plain state and hash all
        // account otherwise take changesets aggregate the sets and apply hashing to
        // AccountHashing table. Also, if we start from genesis, we need to hash from scratch, as
        // genesis accounts are not in changeset, along with their storages.
        if to_block - from_block > self.clean_threshold || from_block == 1 {
            let stage_checkpoint = input
                .checkpoint
                .and_then(|checkpoint| checkpoint.storage_hashing_stage_checkpoint());

            let (mut current_key, mut current_subkey) = match stage_checkpoint {
                Some(StorageHashingCheckpoint {
                         address: address @ Some(_),
                         storage,
                         block_range: CheckpointBlockRange { from, to },
                         ..
                     })
                // Checkpoint is only valid if the range of transitions didn't change.
                // An already hashed storage may have been changed with the new range,
                // and therefore should be hashed again.
                if from == from_block && to == to_block =>
                    {
                        debug!(target: "sync::stages::storage_hashing::exec", checkpoint = ?stage_checkpoint, "Continuing inner storage hashing checkpoint");

                        (address, storage)
                    }
                _ => {
                    // clear table, load all accounts and hash it
                    tx.clear::<tables::HashedStorage>()?;

                    (None, None)
                }
            };

            let mut keccak_address = None;

            let mut hashed_batch = BTreeMap::new();
            let mut remaining = self.commit_threshold as usize;
            {
                let mut storage = tx.cursor_dup_read::<tables::PlainStorageState>()?;
                while !remaining.is_zero() {
                    hashed_batch.extend(
                        storage
                            .walk_dup(current_key, current_subkey)?
                            .take(remaining)
                            .map(|res| {
                                res.map(|(address, slot)| {
                                    // Address caching for the first iteration when current_key
                                    // is None
                                    let keccak_address =
                                        if let Some(keccak_address) = keccak_address {
                                            keccak_address
                                        } else {
                                            keccak256(address)
                                        };

                                    // TODO cache map keccak256(slot.key) ?
                                    ((keccak_address, keccak256(slot.key)), slot.value)
                                })
                            })
                            .collect::<Result<BTreeMap<_, _>, _>>()?,
                    );

                    remaining = self.commit_threshold as usize - hashed_batch.len();

                    if let Some((address, slot)) = storage.next_dup()? {
                        // There's still some remaining elements on this key, so we need to save
                        // the cursor position for the next
                        // iteration
                        (current_key, current_subkey) = (Some(address), Some(slot.key));
                    } else {
                        // Go to the next key
                        (current_key, current_subkey) = storage
                            .next_no_dup()?
                            .map(|(key, storage_entry)| (key, storage_entry.key))
                            .unzip();

                        // Cache keccak256(address) for the next key if it exists
                        if let Some(address) = current_key {
                            keccak_address = Some(keccak256(address));
                        } else {
                            // We have reached the end of table
                            break
                        }
                    }
                }
            }

            // iterate and put presorted hashed slots
            hashed_batch.into_iter().try_for_each(|((addr, key), value)| {
                tx.put::<tables::HashedStorage>(addr, StorageEntry { key, value })
            })?;

            if current_key.is_some() {
                let checkpoint = input.checkpoint().with_storage_hashing_stage_checkpoint(
                    StorageHashingCheckpoint {
                        address: current_key,
                        storage: current_subkey,
                        block_range: CheckpointBlockRange { from: from_block, to: to_block },
                        progress: stage_checkpoint_progress(tx)?,
                    },
                );

                info!(target: "sync::stages::hashing_storage", checkpoint = %checkpoint, is_final_range = false, "Stage iteration finished");
                return Ok(ExecOutput { checkpoint, done: false })
            }
        } else {
            // Aggregate all changesets and and make list of storages that have been
            // changed.
            let lists = tx.get_addresses_and_keys_of_changed_storages(from_block..=to_block)?;
            // iterate over plain state and get newest storage value.
            // Assumption we are okay with is that plain state represent
            // `previous_stage_progress` state.
            let storages = tx.get_plainstate_storages(lists)?;
            tx.insert_storage_for_hashing(storages.into_iter())?;
        }

        // We finished the hashing stage, no future iterations is expected for the same block range,
        // so no checkpoint is needed.
        let checkpoint = input.previous_stage_checkpoint().with_storage_hashing_stage_checkpoint(
            StorageHashingCheckpoint {
                progress: stage_checkpoint_progress(tx)?,
                ..Default::default()
            },
        );

        info!(target: "sync::stages::hashing_storage", checkpoint = %checkpoint, is_final_range = true, "Stage iteration finished");
        Ok(ExecOutput { checkpoint, done: true })
    }

    /// Unwind the stage.
    async fn unwind(
        &mut self,
        tx: &mut Transaction<'_, DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        let (range, unwind_progress, is_final_range) =
            input.unwind_block_range_with_threshold(self.commit_threshold);

        tx.unwind_storage_hashing(BlockNumberAddress::range(range))?;

        let mut stage_checkpoint =
            input.checkpoint.storage_hashing_stage_checkpoint().unwrap_or_default();

        stage_checkpoint.progress = stage_checkpoint_progress(tx)?;

        info!(target: "sync::stages::hashing_storage", to_block = input.unwind_to, %unwind_progress, is_final_range, "Unwind iteration finished");
        Ok(UnwindOutput {
            checkpoint: StageCheckpoint::new(unwind_progress)
                .with_storage_hashing_stage_checkpoint(stage_checkpoint),
        })
    }
}

fn stage_checkpoint_progress<DB: Database>(
    tx: &Transaction<'_, DB>,
) -> Result<EntitiesCheckpoint, DatabaseError> {
    Ok(EntitiesCheckpoint {
        processed: tx.deref().entries::<tables::HashedStorage>()? as u64,
        total: tx.deref().entries::<tables::PlainStorageState>()? as u64,
    })
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
        cursor::{DbCursorRO, DbCursorRW},
        mdbx::{tx::Tx, WriteMap, RW},
        models::{BlockNumberAddress, StoredBlockBodyIndices},
    };
    use reth_interfaces::test_utils::generators::{
        random_block_range, random_contract_account_range,
    };
    use reth_primitives::{
        stage::StageUnitCheckpoint, Address, SealedBlock, StorageEntry, H256, U256,
    };

    stage_test_suite_ext!(StorageHashingTestRunner, storage_hashing);

    /// Execute with low clean threshold so as to hash whole storage
    #[tokio::test]
    async fn execute_clean_storage_hashing() {
        let (previous_stage, stage_progress) = (500, 100);

        // Set up the runner
        let mut runner = StorageHashingTestRunner::default();

        // set low clean threshold so we hash the whole storage
        runner.set_clean_threshold(1);

        // set low commit threshold so we force each entry to be a tx.commit and make sure we don't
        // hang on one key. Seed execution inserts more than one storage entry per address.
        runner.set_commit_threshold(1);

        let mut input = ExecInput {
            previous_stage: Some((PREV_STAGE_ID, StageCheckpoint::new(previous_stage))),
            checkpoint: Some(StageCheckpoint::new(stage_progress)),
        };

        runner.seed_execution(input).expect("failed to seed execution");

        loop {
            if let Ok(result @ ExecOutput { checkpoint, done }) =
                runner.execute(input).await.unwrap()
            {
                if !done {
                    let previous_checkpoint = input
                        .checkpoint
                        .and_then(|checkpoint| checkpoint.storage_hashing_stage_checkpoint())
                        .unwrap_or_default();
                    assert_matches!(checkpoint.storage_hashing_stage_checkpoint(), Some(StorageHashingCheckpoint {
                        progress: EntitiesCheckpoint {
                            processed,
                            total,
                        },
                        ..
                    }) if processed == previous_checkpoint.progress.processed + 1 &&
                        total == runner.tx.table::<tables::PlainStorageState>().unwrap().len() as u64);

                    // Continue from checkpoint
                    input.checkpoint = Some(checkpoint);
                    continue
                } else {
                    assert!(checkpoint.block_number == previous_stage);
                    assert_matches!(checkpoint.storage_hashing_stage_checkpoint(), Some(StorageHashingCheckpoint {
                        progress: EntitiesCheckpoint {
                            processed,
                            total,
                        },
                        ..
                    }) if processed == total &&
                        total == runner.tx.table::<tables::PlainStorageState>().unwrap().len() as u64);

                    // Validate the stage execution
                    assert!(
                        runner.validate_execution(input, Some(result)).is_ok(),
                        "execution validation"
                    );

                    break
                }
            }
            panic!("Failed execution");
        }
    }

    #[tokio::test]
    async fn execute_clean_storage_hashing_with_commit_threshold() {
        let (previous_stage, stage_progress) = (500, 100);
        // Set up the runner
        let mut runner = StorageHashingTestRunner::default();
        runner.set_clean_threshold(1);
        runner.set_commit_threshold(500);

        let mut input = ExecInput {
            previous_stage: Some((PREV_STAGE_ID, StageCheckpoint::new(previous_stage))),
            checkpoint: Some(StageCheckpoint::new(stage_progress)),
        };

        runner.seed_execution(input).expect("failed to seed execution");

        // first run, hash first half of storages.
        let rx = runner.execute(input);
        let result = rx.await.unwrap();

        let (progress_address, progress_key) = runner
            .tx
            .query(|tx| {
                let (address, entry) = tx
                    .cursor_read::<tables::PlainStorageState>()?
                    .walk(None)?
                    .nth(500)
                    .unwrap()
                    .unwrap();
                Ok((address, entry.key))
            })
            .unwrap();

        assert_matches!(
            result,
            Ok(ExecOutput {
                checkpoint: StageCheckpoint {
                    block_number: 100,
                    stage_checkpoint: Some(StageUnitCheckpoint::Storage(StorageHashingCheckpoint {
                        address: Some(address),
                        storage: Some(storage),
                        block_range: CheckpointBlockRange {
                            from: 101,
                            to: 500,
                        },
                        progress: EntitiesCheckpoint {
                            processed: 500,
                            total
                        }
                    }))
                },
                done: false
            }) if address == progress_address && storage == progress_key &&
                total == runner.tx.table::<tables::PlainStorageState>().unwrap().len() as u64
        );
        assert_eq!(runner.tx.table::<tables::HashedStorage>().unwrap().len(), 500);

        // second run with commit threshold of 2 to check if subkey is set.
        runner.set_commit_threshold(2);
        let result = result.unwrap();
        input.checkpoint = Some(result.checkpoint);
        let rx = runner.execute(input);
        let result = rx.await.unwrap();

        let (progress_address, progress_key) = runner
            .tx
            .query(|tx| {
                let (address, entry) = tx
                    .cursor_read::<tables::PlainStorageState>()?
                    .walk(None)?
                    .nth(502)
                    .unwrap()
                    .unwrap();
                Ok((address, entry.key))
            })
            .unwrap();

        assert_matches!(
            result,
            Ok(ExecOutput {
                checkpoint: StageCheckpoint {
                    block_number: 100,
                    stage_checkpoint: Some(StageUnitCheckpoint::Storage(
                        StorageHashingCheckpoint {
                            address: Some(address),
                            storage: Some(storage),
                            block_range: CheckpointBlockRange {
                                from: 101,
                                to: 500,
                            },
                            progress: EntitiesCheckpoint {
                                processed: 502,
                                total
                            }
                        }
                    ))
                },
                done: false
            }) if address == progress_address && storage == progress_key &&
                total == runner.tx.table::<tables::PlainStorageState>().unwrap().len() as u64
        );
        assert_eq!(runner.tx.table::<tables::HashedStorage>().unwrap().len(), 502);

        // third last run, hash rest of storages.
        runner.set_commit_threshold(1000);
        input.checkpoint = Some(result.unwrap().checkpoint);
        let rx = runner.execute(input);
        let result = rx.await.unwrap();

        assert_matches!(
            result,
            Ok(ExecOutput {
                checkpoint: StageCheckpoint {
                    block_number: 500,
                    stage_checkpoint: Some(StageUnitCheckpoint::Storage(
                        StorageHashingCheckpoint {
                            address: None,
                            storage: None,
                            block_range: CheckpointBlockRange {
                                from: 0,
                                to: 0,
                            },
                            progress: EntitiesCheckpoint {
                                processed,
                                total
                            }
                        }
                    ))
                },
                done: true
            }) if processed == total &&
                total == runner.tx.table::<tables::PlainStorageState>().unwrap().len() as u64
        );
        assert_eq!(
            runner.tx.table::<tables::HashedStorage>().unwrap().len(),
            runner.tx.table::<tables::PlainStorageState>().unwrap().len()
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
            let stage_progress = input.checkpoint().block_number + 1;
            let end = input.previous_stage_checkpoint().block_number;

            let n_accounts = 31;
            let mut accounts = random_contract_account_range(&mut (0..n_accounts));

            let blocks = random_block_range(stage_progress..=end, H256::zero(), 0..3);

            self.tx.insert_headers(blocks.iter().map(|block| &block.header))?;

            let iter = blocks.iter();
            let mut next_tx_num = 0;
            let mut first_tx_num = next_tx_num;
            for progress in iter {
                // Insert last progress data
                let block_number = progress.number;
                self.tx.commit(|tx| {
                    progress.body.iter().try_for_each(
                        |transaction| -> Result<(), reth_db::DatabaseError> {
                            tx.put::<tables::TxHashNumber>(transaction.hash(), next_tx_num)?;
                            tx.put::<tables::Transactions>(
                                next_tx_num,
                                transaction.clone().into(),
                            )?;

                            let (addr, _) = accounts
                                .get_mut(rand::random::<usize>() % n_accounts as usize)
                                .unwrap();

                            for _ in 0..2 {
                                let new_entry = StorageEntry {
                                    key: keccak256([rand::random::<u8>()]),
                                    value: U256::from(rand::random::<u8>() % 30 + 1),
                                };
                                self.insert_storage_entry(
                                    tx,
                                    (block_number, *addr).into(),
                                    new_entry,
                                    progress.header.number == stage_progress,
                                )?;
                            }

                            next_tx_num += 1;
                            Ok(())
                        },
                    )?;

                    // Randomize rewards
                    let has_reward: bool = rand::random();
                    if has_reward {
                        self.insert_storage_entry(
                            tx,
                            (block_number, Address::random()).into(),
                            StorageEntry {
                                key: keccak256("mining"),
                                value: U256::from(rand::random::<u32>()),
                            },
                            progress.header.number == stage_progress,
                        )?;
                    }

                    let body = StoredBlockBodyIndices {
                        first_tx_num,
                        tx_count: progress.body.len() as u64,
                    };

                    first_tx_num = next_tx_num;

                    tx.put::<tables::BlockBodyIndices>(progress.number, body)
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
                let start_block = input.checkpoint().block_number + 1;
                let end_block = output.checkpoint.block_number;
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

        fn set_commit_threshold(&mut self, threshold: u64) {
            self.commit_threshold = threshold;
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
                    let count = tx.cursor_dup_read::<tables::HashedStorage>()?.walk(None)?.count();

                    assert_eq!(count, expected);
                    Ok(())
                })
                .map_err(|e| e.into())
        }

        fn insert_storage_entry(
            &self,
            tx: &Tx<'_, RW, WriteMap>,
            tid_address: BlockNumberAddress,
            entry: StorageEntry,
            hash: bool,
        ) -> Result<(), reth_db::DatabaseError> {
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
            let target_block = input.unwind_to;
            self.tx.commit(|tx| {
                let mut storage_cursor = tx.cursor_dup_write::<tables::PlainStorageState>()?;
                let mut changeset_cursor = tx.cursor_dup_read::<tables::StorageChangeSet>()?;

                let mut rev_changeset_walker = changeset_cursor.walk_back(None)?;

                while let Some((bn_address, entry)) = rev_changeset_walker.next().transpose()? {
                    if bn_address.block_number() < target_block {
                        break
                    }

                    if storage_cursor
                        .seek_by_key_subkey(bn_address.address(), entry.key)?
                        .filter(|e| e.key == entry.key)
                        .is_some()
                    {
                        storage_cursor.delete_current()?;
                    }

                    if entry.value != U256::ZERO {
                        storage_cursor.upsert(bn_address.address(), entry)?;
                    }
                }
                Ok(())
            })?;
            Ok(())
        }
    }
}
