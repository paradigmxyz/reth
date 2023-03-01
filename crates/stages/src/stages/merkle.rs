use crate::{ExecInput, ExecOutput, Stage, StageError, StageId, UnwindInput, UnwindOutput};
use reth_db::{database::Database, tables, transaction::DbTx};
use reth_interfaces::consensus;
use reth_provider::{trie::DBTrieLoader, Transaction};
use std::fmt::Debug;
use tracing::*;

/// The [`StageId`] of the merkle hashing execution stage.
pub const MERKLE_EXECUTION: StageId = StageId("MerkleExecute");

/// The [`StageId`] of the merkle hashing unwind stage.
pub const MERKLE_UNWIND: StageId = StageId("MerkleUnwind");

/// The [`StageId`] of the merkle hashing unwind and execution stage.
pub const MERKLE_BOTH: StageId = StageId("MerkleBoth");

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
#[derive(Debug, Clone)]
pub enum MerkleStage {
    /// The execution portion of the merkle stage.
    Execution {
        /// The threshold for switching from incremental trie building
        /// of changes to whole rebuild. Num of transitions.
        clean_threshold: u64,
    },
    /// The unwind portion of the merkle stage.
    Unwind,

    /// Able to execute and unwind. Used for tests
    #[cfg(any(test, feature = "test-utils"))]
    #[allow(missing_docs)]
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
            #[cfg(any(test, feature = "test-utils"))]
            MerkleStage::Both { .. } => MERKLE_BOTH,
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
            #[cfg(any(test, feature = "test-utils"))]
            MerkleStage::Both { clean_threshold } => *clean_threshold,
        };

        let stage_progress = input.stage_progress.unwrap_or_default();
        let previous_stage_progress = input.previous_stage_progress();

        let from_transition = tx.get_block_transition(stage_progress)?;
        let to_transition = tx.get_block_transition(previous_stage_progress)?;

        let block_root = tx.get_header(previous_stage_progress)?.state_root;

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
            let current_root = tx.get_header(stage_progress)?.state_root;
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

        let target_root = tx.get_header(input.unwind_to)?.state_root;

        // If the merkle stage fails to execute, the trie changes weren't commited
        // and the root stayed the same
        if tx.get::<tables::AccountsTrie>(target_root)?.is_some() {
            info!(target: "sync::stages::merkle::unwind", "Stage skipped");
            return Ok(UnwindOutput { stage_progress: input.unwind_to })
        }

        let loader = DBTrieLoader::default();
        let current_root = tx.get_header(input.stage_progress)?.state_root;

        let from_transition = tx.get_block_transition(input.unwind_to)?;
        let to_transition = tx.get_block_transition(input.stage_progress)?;

        let block_root = loader
            .update_root(tx, current_root, from_transition..to_transition)
            .map_err(|e| StageError::Fatal(Box::new(e)))?;

        if block_root != target_root {
            let unwind_to = input.unwind_to;
            warn!(target: "sync::stages::merkle::unwind", ?unwind_to, got = ?block_root, expected = ?target_root, "Block's root state failed verification");
            return Err(StageError::Validation {
                block: unwind_to,
                error: consensus::Error::BodyStateRootDiff {
                    got: block_root,
                    expected: target_root,
                },
            })
        }

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
        tables,
        transaction::{DbTx, DbTxMut},
    };
    use reth_interfaces::test_utils::generators::{
        random_block, random_block_range, random_contract_account_range, random_transition_range,
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
            let accounts = random_contract_account_range(&mut (0..n_accounts))
                .into_iter()
                .collect::<BTreeMap<_, _>>();

            let SealedBlock { header, body, ommers, withdrawals } =
                random_block(stage_progress, None, Some(0), None);
            let mut header = header.unseal();

            header.state_root =
                self.generate_initial_trie(accounts.iter().map(|(k, v)| (*k, *v)))?;
            let sealed_head = SealedBlock { header: header.seal_slow(), body, ommers, withdrawals };

            let head_hash = sealed_head.hash();
            let mut blocks = vec![sealed_head];

            blocks.extend(random_block_range((stage_progress + 1)..end, head_hash, 0..3));

            self.tx.insert_blocks(blocks.iter(), None)?;

            let (transitions, final_state) = random_transition_range(
                blocks.iter(),
                accounts.into_iter().map(|(addr, acc)| (addr, (acc, Vec::new()))),
                0..3,
                0..256,
            );

            self.tx.insert_transitions(transitions, None)?;

            self.tx.insert_accounts_and_storages(final_state)?;

            let last_block_number = end - 1;
            let root = self.state_root()?;
            self.tx.commit(|tx| {
                let mut last_header = tx.get::<tables::Headers>(last_block_number)?.unwrap();
                last_header.state_root = root;
                tx.put::<tables::Headers>(last_block_number, last_header)
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
            accounts: impl IntoIterator<Item = (Address, Account)>,
        ) -> Result<H256, TestRunnerError> {
            self.tx.insert_accounts_and_storages(
                accounts.into_iter().map(|(addr, acc)| (addr, (acc, std::iter::empty()))),
            )?;

            let loader = DBTrieLoader::default();

            let mut tx = self.tx.inner();
            let root = loader.calculate_root(&tx).expect("couldn't create initial trie");

            tx.commit()?;

            Ok(root)
        }

        fn check_root(&self, previous_stage_progress: u64) -> Result<(), TestRunnerError> {
            if previous_stage_progress != 0 {
                let block_root =
                    self.tx.inner().get_header(previous_stage_progress).unwrap().state_root;
                let root = DBTrieLoader::default().calculate_root(&self.tx.inner()).unwrap();
                assert_eq!(block_root, root);
            }
            Ok(())
        }
    }
}
