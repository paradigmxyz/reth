use crate::{ExecInput, ExecOutput, Stage, StageError, StageId, UnwindInput, UnwindOutput};
use reth_codecs::Compact;
use reth_db::{
    database::Database,
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_interfaces::consensus;
use reth_primitives::{
    hex, trie::StoredSubNode, BlockNumber, MerkleCheckpoint, StageCheckpoint, H256,
};
use reth_provider::Transaction;
use reth_trie::{IntermediateStateRootState, StateRoot, StateRootProgress};
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
        Self::Execution { clean_threshold: 50_000 }
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

    /// Gets the hashing progress
    pub fn get_execution_checkpoint<DB: Database>(
        &self,
        tx: &Transaction<'_, DB>,
    ) -> Result<Option<MerkleCheckpoint>, StageError> {
        let buf =
            tx.get::<tables::SyncStageProgress>(MERKLE_EXECUTION.0.into())?.unwrap_or_default();

        if buf.is_empty() {
            return Ok(None)
        }

        let (checkpoint, _) = MerkleCheckpoint::from_compact(&buf, buf.len());
        Ok(Some(checkpoint))
    }

    /// Saves the hashing progress
    pub fn save_execution_checkpoint<DB: Database>(
        &mut self,
        tx: &Transaction<'_, DB>,
        checkpoint: Option<MerkleCheckpoint>,
    ) -> Result<(), StageError> {
        let mut buf = vec![];
        if let Some(checkpoint) = checkpoint {
            debug!(
                target: "sync::stages::merkle::exec",
                last_account_key = ?checkpoint.last_account_key,
                last_walker_key = ?hex::encode(&checkpoint.last_walker_key),
                "Saving inner merkle checkpoint"
            );
            checkpoint.to_compact(&mut buf);
        }
        tx.put::<tables::SyncStageProgress>(MERKLE_EXECUTION.0.into(), buf)?;
        Ok(())
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
                return Ok(ExecOutput { checkpoint: input.previous_stage_checkpoint(), done: true })
            }
            MerkleStage::Execution { clean_threshold } => *clean_threshold,
            #[cfg(any(test, feature = "test-utils"))]
            MerkleStage::Both { clean_threshold } => *clean_threshold,
        };

        let range = input.next_block_range();
        let (from_block, to_block) = range.clone().into_inner();
        let current_block = input.previous_stage_checkpoint().block_number;

        let block_root = tx.get_header(current_block)?.state_root;

        let mut checkpoint = self.get_execution_checkpoint(tx)?;

        let trie_root = if range.is_empty() {
            block_root
        } else if to_block - from_block > threshold || from_block == 1 {
            // if there are more blocks than threshold it is faster to rebuild the trie
            if let Some(checkpoint) = checkpoint.as_ref().filter(|c| c.target_block == to_block) {
                debug!(
                    target: "sync::stages::merkle::exec",
                    current = ?current_block,
                    target = ?to_block,
                    last_account_key = ?checkpoint.last_account_key,
                    last_walker_key = ?hex::encode(&checkpoint.last_walker_key),
                    "Continuing inner merkle checkpoint"
                );
            } else {
                debug!(
                    target: "sync::stages::merkle::exec",
                    current = ?current_block,
                    target = ?to_block,
                    previous_checkpoint = ?checkpoint,
                    "Rebuilding trie"
                );
                // Reset the checkpoint and clear trie tables
                checkpoint = None;
                self.save_execution_checkpoint(tx, None)?;
                tx.clear::<tables::AccountsTrie>()?;
                tx.clear::<tables::StoragesTrie>()?;
            }

            let progress = StateRoot::new(tx.deref_mut())
                .with_intermediate_state(checkpoint.map(IntermediateStateRootState::from))
                .root_with_progress()
                .map_err(|e| StageError::Fatal(Box::new(e)))?;
            match progress {
                StateRootProgress::Progress(state, updates) => {
                    updates.flush(tx.deref_mut())?;
                    let checkpoint = MerkleCheckpoint::new(
                        to_block,
                        state.last_account_key,
                        state.last_walker_key.hex_data,
                        state.walker_stack.into_iter().map(StoredSubNode::from).collect(),
                        state.hash_builder.into(),
                    );
                    self.save_execution_checkpoint(tx, Some(checkpoint))?;
                    return Ok(ExecOutput { checkpoint: input.checkpoint(), done: false })
                }
                StateRootProgress::Complete(root, updates) => {
                    updates.flush(tx.deref_mut())?;
                    root
                }
            }
        } else {
            debug!(target: "sync::stages::merkle::exec", current = ?current_block, target = ?to_block, "Updating trie");
            let (root, updates) = StateRoot::incremental_root_with_updates(tx.deref_mut(), range)
                .map_err(|e| StageError::Fatal(Box::new(e)))?;
            updates.flush(tx.deref_mut())?;
            root
        };

        // Reset the checkpoint
        self.save_execution_checkpoint(tx, None)?;

        self.validate_state_root(trie_root, block_root, to_block)?;

        info!(target: "sync::stages::merkle::exec", stage_progress = to_block, is_final_range = true, "Stage iteration finished");
        Ok(ExecOutput { checkpoint: StageCheckpoint::new(to_block), done: true })
    }

    /// Unwind the stage.
    async fn unwind(
        &mut self,
        tx: &mut Transaction<'_, DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        let range = input.unwind_block_range();
        if matches!(self, MerkleStage::Execution { .. }) {
            info!(target: "sync::stages::merkle::unwind", "Stage is always skipped");
            return Ok(UnwindOutput { checkpoint: StageCheckpoint::new(input.unwind_to) })
        }

        if input.unwind_to == 0 {
            tx.clear::<tables::AccountsTrie>()?;
            tx.clear::<tables::StoragesTrie>()?;
            info!(target: "sync::stages::merkle::unwind", stage_progress = input.unwind_to, is_final_range = true, "Unwind iteration finished");
            return Ok(UnwindOutput { checkpoint: StageCheckpoint::new(input.unwind_to) })
        }

        // Unwind trie only if there are transitions
        if !range.is_empty() {
            let (block_root, updates) =
                StateRoot::incremental_root_with_updates(tx.deref_mut(), range)
                    .map_err(|e| StageError::Fatal(Box::new(e)))?;

            // Validate the calulated state root
            let target_root = tx.get_header(input.unwind_to)?.state_root;
            self.validate_state_root(block_root, target_root, input.unwind_to)?;

            // Validation passed, apply unwind changes to the database.
            updates.flush(tx.deref_mut())?;
        } else {
            info!(target: "sync::stages::merkle::unwind", "Nothing to unwind");
        }

        info!(target: "sync::stages::merkle::unwind", stage_progress = input.unwind_to, is_final_range = true, "Unwind iteration finished");
        Ok(UnwindOutput { checkpoint: StageCheckpoint::new(input.unwind_to) })
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
            previous_stage: Some((PREV_STAGE_ID, StageCheckpoint::new(previous_stage))),
            checkpoint: Some(StageCheckpoint::new(stage_progress)),
        };

        runner.seed_execution(input).expect("failed to seed execution");

        let rx = runner.execute(input);

        // Assert the successful result
        let result = rx.await.unwrap();
        assert_matches!(
            result,
            Ok(ExecOutput { checkpoint: StageCheckpoint { block_number, .. }, done: true })
                if block_number == previous_stage
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
            previous_stage: Some((PREV_STAGE_ID, StageCheckpoint::new(previous_stage))),
            checkpoint: Some(StageCheckpoint::new(stage_progress)),
        };

        runner.seed_execution(input).expect("failed to seed execution");

        let rx = runner.execute(input);

        // Assert the successful result
        let result = rx.await.unwrap();
        assert_matches!(
            result,
            Ok(ExecOutput { checkpoint: StageCheckpoint { block_number, .. }, done: true })
                if block_number == previous_stage
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
            let stage_progress = input.checkpoint().block_number;
            let start = stage_progress + 1;
            let end = input.previous_stage_checkpoint().block_number;

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
            blocks.extend(random_block_range(start..=end, head_hash, 0..3));
            self.tx.insert_blocks(blocks.iter(), None)?;

            let (transitions, final_state) = random_transition_range(
                blocks.iter(),
                accounts.into_iter().map(|(addr, acc)| (addr, (acc, Vec::new()))),
                0..3,
                0..256,
            );
            // add block changeset from block 1.
            self.tx.insert_transitions(transitions, Some(start))?;
            self.tx.insert_accounts_and_storages(final_state)?;

            // Calculate state root
            let root = self.tx.query(|tx| {
                let mut accounts = BTreeMap::default();
                let mut accounts_cursor = tx.cursor_read::<tables::HashedAccount>()?;
                let mut storage_cursor = tx.cursor_dup_read::<tables::HashedStorage>()?;
                for entry in accounts_cursor.walk_range(..)? {
                    let (key, account) = entry?;
                    let mut storage_entries = Vec::new();
                    let mut entry = storage_cursor.seek_exact(key)?;
                    while let Some((_, storage)) = entry {
                        storage_entries.push(storage);
                        entry = storage_cursor.next_dup()?;
                    }
                    let storage = storage_entries
                        .into_iter()
                        .filter(|v| v.value != U256::ZERO)
                        .map(|v| (v.key, v.value))
                        .collect::<Vec<_>>();
                    accounts.insert(key, (account, storage));
                }

                Ok(state_root_prehashed(accounts.into_iter()))
            })?;

            let last_block_number = end;
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
            let target_block = input.unwind_to + 1;

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
                        if tid_address.block_number() < target_block {
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

                    while let Some((block_number, account_before_tx)) =
                        rev_changeset_walker.next().transpose().unwrap()
                    {
                        if block_number < target_block {
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
