use crate::{
    db::Transaction, trie::DBTrieLoader, ExecInput, ExecOutput, Stage, StageError, StageId,
    UnwindInput, UnwindOutput,
};
use reth_db::{database::Database, tables, transaction::DbTx};
use reth_interfaces::consensus;
use std::fmt::Debug;
use tracing::*;

/// The [`StageId`] of the merkle hashing execution stage.
pub const MERKLE_EXECUTION: StageId = StageId("MerkleExecute");

/// The [`StageId`] of the merkle hashing unwind stage.
pub const MERKLE_UNWIND: StageId = StageId("MerkleUnwind");

/// The merkle hashing stage uses input from
/// [`AccountHashingStage`][crate::stages::AccountHashingStage] and
/// [`StorageHashingStage`][crate::stages::AccountHashingStage] to calculate intermediate hashes
/// and state roots.
///
/// This stage should be run with the above two stages, otherwise it is a no-op.
///
/// This stage is split in two: one for calculating hashes and one for unwinding.
///
/// When run in execution, it's going to be executed AFTER the hashing stages, to generate
/// the state root. When run in unwind mode, it's going to be executed BEFORE the hashing stages,
/// so that it unwinds the intermediate hashes based on the unwound hashed state from the hashing
/// stages. The order of these two variants is important. The unwind variant should be added to the
/// pipeline before the execution variant.
///
/// An example pipeline to only hash state would be:
///
/// - [`MerkleStage::Unwind`]
/// - [`AccountHashingStage`][crate::stages::AccountHashingStage]
/// - [`StorageHashingStage`][crate::stages::StorageHashingStage]
/// - [`MerkleStage::Execution`]
#[derive(Debug)]
pub enum MerkleStage {
    /// The execution portion of the merkle stage.
    Execution {
        /// The threshold for switching from incremental trie building
        /// of changes to whole rebuild. Num of transitions.
        clean_threshold: u64,
    },
    /// The unwind portion of the merkle stage.
    Unwind,

    #[cfg(test)]
    Both { clean_threshold: u64 },
}

impl MerkleStage {
    /// Stage default for the Execution variant.
    pub fn default_execution() -> Self {
        Self::Execution { clean_threshold: 5_000 }
    }

    /// Stage default for the Unwind variant.
    pub fn default_unwind() -> Self {
        Self::Unwind
    }
}

#[async_trait::async_trait]
impl<DB: Database> Stage<DB> for MerkleStage {
    /// Return the id of the stage
    fn id(&self) -> StageId {
        match self {
            MerkleStage::Execution { .. } => MERKLE_EXECUTION,
            MerkleStage::Unwind => MERKLE_UNWIND,
            #[cfg(test)]
            MerkleStage::Both { .. } => unreachable!(),
        }
    }

    /// Execute the stage.
    async fn execute(
        &mut self,
        tx: &mut Transaction<'_, DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        let threshold = match self {
            MerkleStage::Unwind => {
                info!(target: "sync::stages::merkle::unwind", "Stage is always skipped");
                return Ok(ExecOutput {
                    stage_progress: input.previous_stage_progress(),
                    done: true,
                })
            }
            MerkleStage::Execution { clean_threshold } => *clean_threshold,
            #[cfg(test)]
            MerkleStage::Both { clean_threshold } => *clean_threshold,
        };

        let stage_progress = input.stage_progress.unwrap_or_default();
        let previous_stage_progress = input.previous_stage_progress();

        let from_transition = tx.get_block_transition(stage_progress)?;
        let to_transition = tx.get_block_transition(previous_stage_progress)?;

        let block_root = tx.get_header_by_num(previous_stage_progress)?.state_root;

        let trie_root = if from_transition == to_transition {
            block_root
        } else if to_transition - from_transition > threshold || stage_progress == 0 {
            debug!(target: "sync::stages::merkle::exec", current = ?stage_progress, target = ?previous_stage_progress, "Rebuilding trie");
            // if there are more blocks than threshold it is faster to rebuild the trie
            let loader = DBTrieLoader::default();
            loader.calculate_root(tx).map_err(|e| StageError::Fatal(Box::new(e)))?
        } else {
            debug!(target: "sync::stages::merkle::exec", current = ?stage_progress, target = ?previous_stage_progress, "Updating trie");
            // Iterate over changeset (similar to Hashing stages) and take new values
            let current_root = tx.get_header_by_num(stage_progress)?.state_root;
            let loader = DBTrieLoader::default();
            loader
                .update_root(tx, current_root, from_transition..to_transition)
                .map_err(|e| StageError::Fatal(Box::new(e)))?
        };

        if block_root != trie_root {
            warn!(target: "sync::stages::merkle::exec", ?previous_stage_progress, got = ?block_root, expected = ?trie_root, "Block's root state failed verification");
            return Err(StageError::Validation {
                block: previous_stage_progress,
                error: consensus::Error::BodyStateRootDiff { got: trie_root, expected: block_root },
            })
        }

        info!(target: "sync::stages::merkle::exec", "Stage finished");
        Ok(ExecOutput { stage_progress: input.previous_stage_progress(), done: true })
    }

    /// Unwind the stage.
    async fn unwind(
        &mut self,
        tx: &mut Transaction<'_, DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        if matches!(self, MerkleStage::Execution { .. }) {
            info!(target: "sync::stages::merkle::exec", "Stage is always skipped");
            return Ok(UnwindOutput { stage_progress: input.unwind_to })
        }

        let target_root = tx.get_header_by_num(input.unwind_to)?.state_root;

        // If the merkle stage fails to execute, the trie changes weren't commited
        // and the root stayed the same
        if tx.get::<tables::AccountsTrie>(target_root)?.is_some() {
            info!(target: "sync::stages::merkle::unwind", "Stage skipped");
            return Ok(UnwindOutput { stage_progress: input.unwind_to })
        }

        let loader = DBTrieLoader::default();
        let current_root = tx.get_header_by_num(input.stage_progress)?.state_root;

        let from_transition = tx.get_block_transition(input.unwind_to)?;
        let to_transition = tx.get_block_transition(input.stage_progress)?;

        loader
            .update_root(tx, current_root, from_transition..to_transition)
            .map_err(|e| StageError::Fatal(Box::new(e)))?;

        info!(target: "sync::stages::merkle::unwind", "Stage finished");
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
        cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW},
        models::{AccountBeforeTx, BlockNumHash, StoredBlockBody},
        tables,
        transaction::{DbTx, DbTxMut},
    };
    use reth_interfaces::test_utils::generators::{
        random_block, random_block_range, random_contract_account_range,
    };
    use reth_primitives::{keccak256, Account, Address, SealedBlock, StorageEntry, H256, U256};
    use std::collections::BTreeMap;

    stage_test_suite_ext!(MerkleTestRunner, merkle);

    /// Execute from genesis so as to merkelize whole state
    #[tokio::test]
    async fn execute_clean_merkle() {
        let (previous_stage, stage_progress) = (500, 0);

        // Set up the runner
        let mut runner = MerkleTestRunner::default();
        // set low threshold so we hash the whole storage
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

    /// Update small trie
    #[tokio::test]
    async fn execute_small_merkle() {
        let (previous_stage, stage_progress) = (2, 1);

        // Set up the runner
        let mut runner = MerkleTestRunner::default();
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

    struct MerkleTestRunner {
        tx: TestTransaction,
        clean_threshold: u64,
    }

    impl Default for MerkleTestRunner {
        fn default() -> Self {
            Self { tx: TestTransaction::default(), clean_threshold: 10000 }
        }
    }

    impl StageTestRunner for MerkleTestRunner {
        type S = MerkleStage;

        fn tx(&self) -> &TestTransaction {
            &self.tx
        }

        fn stage(&self) -> Self::S {
            Self::S::Both { clean_threshold: self.clean_threshold }
        }
    }

    #[async_trait::async_trait]
    impl ExecuteStageTestRunner for MerkleTestRunner {
        type Seed = Vec<SealedBlock>;

        fn seed_execution(&mut self, input: ExecInput) -> Result<Self::Seed, TestRunnerError> {
            let stage_progress = input.stage_progress.unwrap_or_default();
            let end = input.previous_stage_progress() + 1;

            let n_accounts = 31;
            let mut accounts = random_contract_account_range(&mut (0..n_accounts));

            let SealedBlock { header, body, ommers } =
                random_block(stage_progress, None, Some(0), None);
            let mut header = header.unseal();
            header.state_root = self.generate_initial_trie(&accounts)?;
            let sealed_head = SealedBlock { header: header.seal(), body, ommers };

            let head_hash = sealed_head.hash();
            let mut blocks = vec![sealed_head];

            blocks.extend(random_block_range((stage_progress + 1)..end, head_hash, 0..3));

            self.tx.insert_headers(blocks.iter().map(|block| &block.header))?;

            let (mut transition_id, mut tx_id) = (0, 0);

            let mut storages: BTreeMap<Address, BTreeMap<H256, U256>> = BTreeMap::new();

            for progress in blocks.iter() {
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

                        // seed account changeset
                        let (addr, prev_acc) = accounts
                            .get_mut(rand::random::<usize>() % n_accounts as usize)
                            .unwrap();
                        let acc_before_tx =
                            AccountBeforeTx { address: *addr, info: Some(*prev_acc) };

                        tx.put::<tables::AccountChangeSet>(transition_id, acc_before_tx)?;

                        prev_acc.nonce += 1;
                        prev_acc.balance = prev_acc.balance.wrapping_add(U256::from(1));

                        let new_entry = StorageEntry {
                            key: keccak256([rand::random::<u8>()]),
                            value: U256::from(rand::random::<u8>() % 30 + 1),
                        };
                        let storage = storages.entry(*addr).or_default();
                        let old_value = storage.entry(new_entry.key).or_default();

                        tx.put::<tables::StorageChangeSet>(
                            (transition_id, *addr).into(),
                            StorageEntry { key: new_entry.key, value: *old_value },
                        )?;

                        *old_value = new_entry.value;

                        tx_id += 1;
                        transition_id += 1;

                        Ok(())
                    })?;

                    tx.put::<tables::BlockTransitionIndex>(key.number(), transition_id)?;
                    tx.put::<tables::BlockBodies>(key, body)
                })?;
            }

            self.insert_accounts(&accounts)?;
            self.insert_storages(&storages)?;

            let last_numhash = self.tx.inner().get_block_numhash(end - 1).unwrap();
            let root = self.state_root()?;
            self.tx.commit(|tx| {
                let mut last_header = tx.get::<tables::Headers>(last_numhash)?.unwrap();
                last_header.state_root = root;
                tx.put::<tables::Headers>(last_numhash, last_header)
            })?;

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
            self.check_root(input.previous_stage_progress())
        }
    }

    impl UnwindStageTestRunner for MerkleTestRunner {
        fn before_unwind(&self, input: UnwindInput) -> Result<(), TestRunnerError> {
            let target_transition = self
                .tx
                .inner()
                .get_block_transition(input.unwind_to)
                .map_err(|e| TestRunnerError::Internal(Box::new(e)))
                .unwrap();

            self.tx
                .commit(|tx| {
                    let mut changeset_cursor =
                        tx.cursor_dup_read::<tables::StorageChangeSet>().unwrap();
                    let mut hash_cursor = tx.cursor_dup_write::<tables::HashedStorage>().unwrap();

                    let mut rev_changeset_walker = changeset_cursor.walk_back(None).unwrap();

                    let mut tree: BTreeMap<H256, BTreeMap<H256, U256>> = BTreeMap::new();

                    while let Some((tid_address, entry)) =
                        rev_changeset_walker.next().transpose().unwrap()
                    {
                        if tid_address.transition_id() < target_transition {
                            break
                        }

                        tree.entry(keccak256(tid_address.address()))
                            .or_default()
                            .insert(keccak256(entry.key), entry.value);
                    }
                    for (key, val) in tree.into_iter() {
                        for (entry_key, entry_val) in val.into_iter() {
                            hash_cursor.seek_by_key_subkey(key, entry_key).unwrap();
                            hash_cursor.delete_current().unwrap();

                            if entry_val != U256::ZERO {
                                let storage_entry =
                                    StorageEntry { key: entry_key, value: entry_val };
                                hash_cursor.append_dup(key, storage_entry).unwrap();
                            }
                        }
                    }

                    let mut changeset_cursor =
                        tx.cursor_dup_write::<tables::AccountChangeSet>().unwrap();
                    let mut rev_changeset_walker = changeset_cursor.walk_back(None).unwrap();

                    while let Some((transition_id, account_before_tx)) =
                        rev_changeset_walker.next().transpose().unwrap()
                    {
                        if transition_id < target_transition {
                            break
                        }

                        match account_before_tx.info {
                            Some(acc) => {
                                tx.put::<tables::PlainAccountState>(account_before_tx.address, acc)
                                    .unwrap();
                                tx.put::<tables::HashedAccount>(
                                    keccak256(account_before_tx.address),
                                    acc,
                                )
                                .unwrap();
                            }
                            None => {
                                tx.delete::<tables::PlainAccountState>(
                                    account_before_tx.address,
                                    None,
                                )
                                .unwrap();
                                tx.delete::<tables::HashedAccount>(
                                    keccak256(account_before_tx.address),
                                    None,
                                )
                                .unwrap();
                            }
                        }
                    }
                    Ok(())
                })
                .unwrap();
            Ok(())
        }

        fn validate_unwind(&self, input: UnwindInput) -> Result<(), TestRunnerError> {
            self.check_root(input.unwind_to)
        }
    }

    impl MerkleTestRunner {
        fn state_root(&self) -> Result<H256, TestRunnerError> {
            Ok(DBTrieLoader::default().calculate_root(&self.tx.inner()).unwrap())
        }

        pub(crate) fn generate_initial_trie(
            &self,
            accounts: &[(Address, Account)],
        ) -> Result<H256, TestRunnerError> {
            self.insert_accounts(accounts)?;

            let loader = DBTrieLoader::default();

            let mut tx = self.tx.inner();
            let root = loader.calculate_root(&tx).expect("couldn't create initial trie");

            tx.commit()?;

            Ok(root)
        }

        pub(crate) fn insert_accounts(
            &self,
            accounts: &[(Address, Account)],
        ) -> Result<(), TestRunnerError> {
            for (addr, acc) in accounts.iter() {
                self.tx.commit(|tx| {
                    tx.put::<tables::PlainAccountState>(*addr, *acc)?;
                    tx.put::<tables::HashedAccount>(keccak256(addr), *acc)?;
                    Ok(())
                })?;
            }

            Ok(())
        }

        fn insert_storages(
            &self,
            storages: &BTreeMap<Address, BTreeMap<H256, U256>>,
        ) -> Result<(), TestRunnerError> {
            self.tx
                .commit(|tx| {
                    storages.iter().try_for_each(|(&addr, storage)| {
                        storage.iter().try_for_each(|(&key, &value)| {
                            let entry = StorageEntry { key, value };
                            tx.put::<tables::PlainStorageState>(addr, entry)
                        })
                    })?;
                    storages
                        .iter()
                        .map(|(addr, storage)| {
                            (
                                keccak256(addr),
                                storage
                                    .iter()
                                    .filter(|(_, &value)| value != U256::ZERO)
                                    .map(|(key, value)| (keccak256(key), value)),
                            )
                        })
                        .collect::<BTreeMap<_, _>>()
                        .into_iter()
                        .try_for_each(|(addr, storage)| {
                            storage.into_iter().try_for_each(|(key, &value)| {
                                let entry = StorageEntry { key, value };
                                tx.put::<tables::HashedStorage>(addr, entry)
                            })
                        })?;
                    Ok(())
                })
                .map_err(|e| e.into())
        }

        fn check_root(&self, previous_stage_progress: u64) -> Result<(), TestRunnerError> {
            if previous_stage_progress != 0 {
                let block_root =
                    self.tx.inner().get_header_by_num(previous_stage_progress).unwrap().state_root;
                let root = DBTrieLoader::default().calculate_root(&self.tx.inner()).unwrap();
                assert_eq!(block_root, root);
            }
            Ok(())
        }
    }
}
