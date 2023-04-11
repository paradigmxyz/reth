use crate::{ExecInput, ExecOutput, Stage, StageError, StageId, UnwindInput, UnwindOutput};
use reth_db::{database::Database, tables, transaction::DbTxMut};
use reth_interfaces::consensus;
use reth_primitives::{BlockNumber, H256};
use reth_provider::Transaction;
use reth_trie::StateRoot;
use std::{fmt::Debug, ops::DerefMut};
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

    /// Check that the computed state root matches the expected.
    fn validate_state_root(
        &self,
        got: H256,
        expected: H256,
        target_block: BlockNumber,
    ) -> Result<(), StageError> {
        if got == expected {
            Ok(())
        } else {
            warn!(target: "sync::stages::merkle", ?target_block, ?got, ?expected, "Block's root state failed verification");
            Err(StageError::Validation {
                block: target_block,
                error: consensus::ConsensusError::BodyStateRootDiff { got, expected },
            })
        }
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
            // if there are more blocks than threshold it is faster to rebuild the trie
            debug!(target: "sync::stages::merkle::exec", current = ?stage_progress, target = ?previous_stage_progress, "Rebuilding trie");
            tx.clear::<tables::AccountsTrie>()?;
            tx.clear::<tables::StoragesTrie>()?;
            StateRoot::new(tx.deref_mut()).root(None).map_err(|e| StageError::Fatal(Box::new(e)))?
        } else {
            debug!(target: "sync::stages::merkle::exec", current = ?stage_progress, target =
                ?previous_stage_progress, "Updating trie"); // Iterate over
            StateRoot::incremental_root(tx.deref_mut(), from_transition..to_transition, None)
                .map_err(|e| StageError::Fatal(Box::new(e)))?
        };

        self.validate_state_root(trie_root, block_root, previous_stage_progress)?;

        info!(target: "sync::stages::merkle::exec", "Stage finished");
        Ok(ExecOutput { stage_progress: previous_stage_progress, done: true })
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

        if input.unwind_to == 0 {
            tx.clear::<tables::AccountsTrie>()?;
            tx.clear::<tables::StoragesTrie>()?;
            return Ok(UnwindOutput { stage_progress: input.unwind_to })
        }

        let from_transition = tx.get_block_transition(input.unwind_to)?;
        let to_transition = tx.get_block_transition(input.stage_progress)?;

        // Unwind trie only if there are transitions
        if from_transition < to_transition {
            let block_root =
                StateRoot::incremental_root(tx.deref_mut(), from_transition..to_transition, None)
                    .map_err(|e| StageError::Fatal(Box::new(e)))?;
            let target_root = tx.get_header(input.unwind_to)?.state_root;
            self.validate_state_root(block_root, target_root, input.unwind_to)?;
        } else {
            info!(target: "sync::stages::merkle::unwind", "Nothing to unwind");
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
        cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO},
        tables,
        transaction::{DbTx, DbTxMut},
    };
    use reth_interfaces::test_utils::generators::{
        random_block, random_block_range, random_contract_account_range, random_transition_range,
    };
    use reth_primitives::{keccak256, SealedBlock, StorageEntry, H256, U256};
    use reth_trie::test_utils::{state_root, state_root_prehashed};
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

            let num_of_accounts = 31;
            let accounts = random_contract_account_range(&mut (0..num_of_accounts))
                .into_iter()
                .collect::<BTreeMap<_, _>>();

            self.tx.insert_accounts_and_storages(
                accounts.iter().map(|(addr, acc)| (*addr, (*acc, std::iter::empty()))),
            )?;

            let SealedBlock { header, body, ommers, withdrawals } =
                random_block(stage_progress, None, Some(0), None);
            let mut header = header.unseal();

            header.state_root = state_root(
                accounts
                    .clone()
                    .into_iter()
                    .map(|(address, account)| (address, (account, std::iter::empty()))),
            );
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

            // Calculate state root
            let root = self.tx.query(|tx| {
                let mut accounts = BTreeMap::default();
                let mut accounts_cursor = tx.cursor_read::<tables::HashedAccount>()?;
                let mut storage_cursor = tx.cursor_dup_read::<tables::HashedStorage>()?;
                for entry in accounts_cursor.walk_range(..)? {
                    let (key, account) = entry?;
                    let storage_entries =
                        storage_cursor.walk_dup(Some(key), None)?.collect::<Result<Vec<_>, _>>()?;
                    let storage = storage_entries
                        .into_iter()
                        .filter(|(_, v)| v.value != U256::ZERO)
                        .map(|(_, v)| (v.key, v.value))
                        .collect::<Vec<_>>();
                    accounts.insert(key, (account, storage));
                }

                Ok(state_root_prehashed(accounts.into_iter()))
            })?;

            let last_block_number = end - 1;
            self.tx.commit(|tx| {
                let mut last_header = tx.get::<tables::Headers>(last_block_number)?.unwrap();
                last_header.state_root = root;
                tx.put::<tables::Headers>(last_block_number, last_header)
            })?;

            Ok(blocks)
        }

        fn validate_execution(
            &self,
            _input: ExecInput,
            _output: Option<ExecOutput>,
        ) -> Result<(), TestRunnerError> {
            // The execution is validated within the stage
            Ok(())
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
                    let mut storage_changesets_cursor =
                        tx.cursor_dup_read::<tables::StorageChangeSet>().unwrap();
                    let mut storage_cursor =
                        tx.cursor_dup_write::<tables::HashedStorage>().unwrap();

                    let mut tree: BTreeMap<H256, BTreeMap<H256, U256>> = BTreeMap::new();

                    let mut rev_changeset_walker =
                        storage_changesets_cursor.walk_back(None).unwrap();
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
                    for (hashed_address, storage) in tree.into_iter() {
                        for (hashed_slot, value) in storage.into_iter() {
                            let storage_entry = storage_cursor
                                .seek_by_key_subkey(hashed_address, hashed_slot)
                                .unwrap();
                            if storage_entry.map(|v| v.key == hashed_slot).unwrap_or_default() {
                                storage_cursor.delete_current().unwrap();
                            }

                            if value != U256::ZERO {
                                let storage_entry = StorageEntry { key: hashed_slot, value };
                                storage_cursor.upsert(hashed_address, storage_entry).unwrap();
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

                        if let Some(acc) = account_before_tx.info {
                            tx.put::<tables::HashedAccount>(
                                keccak256(account_before_tx.address),
                                acc,
                            )
                            .unwrap();
                        } else {
                            tx.delete::<tables::HashedAccount>(
                                keccak256(account_before_tx.address),
                                None,
                            )
                            .unwrap();
                        }
                    }
                    Ok(())
                })
                .unwrap();
            Ok(())
        }

        fn validate_unwind(&self, _input: UnwindInput) -> Result<(), TestRunnerError> {
            // The unwind is validated within the stage
            Ok(())
        }
    }
}
