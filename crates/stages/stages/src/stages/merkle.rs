use alloy_consensus::BlockHeader;
use alloy_primitives::{BlockNumber, Sealable, B256};
use reth_codecs::Compact;
use reth_consensus::ConsensusError;
use reth_db_api::{
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_primitives_traits::{GotExpected, SealedHeader};
use reth_provider::{
    ChangeSetReader, DBProvider, HeaderProvider, ProviderError, StageCheckpointReader,
    StageCheckpointWriter, StatsReader, StorageChangeSetReader, StorageSettingsCache, TrieWriter,
};
use reth_stages_api::{
    BlockErrorKind, CheckpointBlockRange, EntitiesCheckpoint, ExecInput, ExecOutput,
    ExecutionCheckpoint, MerkleCheckpoint, Stage, StageCheckpoint, StageError, StageId,
    StorageRootMerkleCheckpoint, UnwindInput, UnwindOutput,
};
use reth_trie::{IntermediateStateRootState, StateRoot, StateRootProgress, StoredSubNode};
use reth_trie_db::DatabaseStateRoot;

use std::fmt::Debug;

type DbStateRoot<'a, TX, A> = StateRoot<
    reth_trie_db::DatabaseTrieCursorFactory<&'a TX, A>,
    reth_trie_db::DatabaseHashedCursorFactory<&'a TX>,
>;
use tracing::*;

// TODO: automate the process outlined below so the user can just send in a debugging package
/// The error message that we include in invalid state root errors to tell users what information
/// they should include in a bug report, since true state root errors can be impossible to debug
/// with just basic logs.
pub const INVALID_STATE_ROOT_ERROR_MESSAGE: &str = r#"
Invalid state root error on stage verification!
This is an error that likely requires a report to the reth team with additional information.
Please include the following information in your report:
 * This error message
 * The state root of the block that was rejected
 * The output of `reth db stats --checksum` from the database that was being used. This will take a long time to run!
 * 50-100 lines of logs before and after the first occurrence of the log message with the state root of the block that was rejected.
 * The debug logs from __the same time period__. To find the default location for these logs, run:
   `reth --help | grep -A 4 'log.file.directory'`

Once you have this information, please submit a github issue at https://github.com/paradigmxyz/reth/issues/new
"#;

/// The default threshold (in number of blocks) for switching from incremental trie building
/// of changes to whole rebuild.
pub const MERKLE_STAGE_DEFAULT_REBUILD_THRESHOLD: u64 = 100_000;

/// The default threshold (in number of blocks) to run the stage in incremental mode. The
/// incremental mode will calculate the state root for a large range of blocks by calculating the
/// new state root for this many blocks, in batches, repeating until we reach the desired block
/// number.
pub const MERKLE_STAGE_DEFAULT_INCREMENTAL_THRESHOLD: u64 = 7_000;

/// The merkle hashing stage uses input from
/// [`AccountHashingStage`][crate::stages::AccountHashingStage] and
/// [`StorageHashingStage`][crate::stages::StorageHashingStage] to calculate intermediate hashes
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
        // TODO: make struct for holding incremental settings, for code reuse between `Execution`
        // variant and `Both`
        /// The threshold (in number of blocks) for switching from incremental trie building
        /// of changes to whole rebuild.
        rebuild_threshold: u64,
        /// The threshold (in number of blocks) to run the stage in incremental mode. The
        /// incremental mode will calculate the state root by calculating the new state root for
        /// some number of blocks, repeating until we reach the desired block number.
        incremental_threshold: u64,
    },
    /// The unwind portion of the merkle stage.
    Unwind {
        /// Whether every child of a changed branch path should be walked.
        walk_all_changed_branch_children: bool,
    },
    /// Able to execute and unwind. Used for tests
    #[cfg(any(test, feature = "test-utils"))]
    Both {
        /// The threshold (in number of blocks) for switching from incremental trie building
        /// of changes to whole rebuild.
        rebuild_threshold: u64,
        /// The threshold (in number of blocks) to run the stage in incremental mode. The
        /// incremental mode will calculate the state root by calculating the new state root for
        /// some number of blocks, repeating until we reach the desired block number.
        incremental_threshold: u64,
    },
}

impl MerkleStage {
    /// Stage default for the [`MerkleStage::Execution`].
    pub const fn default_execution() -> Self {
        Self::Execution {
            rebuild_threshold: MERKLE_STAGE_DEFAULT_REBUILD_THRESHOLD,
            incremental_threshold: MERKLE_STAGE_DEFAULT_INCREMENTAL_THRESHOLD,
        }
    }

    /// Stage default for the [`MerkleStage::Unwind`].
    pub const fn default_unwind() -> Self {
        Self::new_unwind(false)
    }

    /// Create a new instance of [`MerkleStage::Unwind`].
    pub const fn new_unwind(walk_all_changed_branch_children: bool) -> Self {
        Self::Unwind { walk_all_changed_branch_children }
    }

    /// Create new instance of [`MerkleStage::Execution`].
    pub const fn new_execution(rebuild_threshold: u64, incremental_threshold: u64) -> Self {
        Self::Execution { rebuild_threshold, incremental_threshold }
    }

    /// Gets the hashing progress
    pub fn get_execution_checkpoint(
        &self,
        provider: &impl StageCheckpointReader,
    ) -> Result<Option<MerkleCheckpoint>, StageError> {
        let buf =
            provider.get_stage_checkpoint_progress(StageId::MerkleExecute)?.unwrap_or_default();

        if buf.is_empty() {
            return Ok(None)
        }

        let (checkpoint, _) = MerkleCheckpoint::from_compact(&buf, buf.len());
        Ok(Some(checkpoint))
    }

    /// Saves the hashing progress
    pub fn save_execution_checkpoint(
        &self,
        provider: &impl StageCheckpointWriter,
        checkpoint: Option<MerkleCheckpoint>,
    ) -> Result<(), StageError> {
        let mut buf = vec![];
        if let Some(checkpoint) = checkpoint {
            debug!(
                target: "sync::stages::merkle::exec",
                last_account_key = ?checkpoint.last_account_key,
                "Saving inner merkle checkpoint"
            );
            checkpoint.to_compact(&mut buf);
        }
        Ok(provider.save_stage_checkpoint_progress(StageId::MerkleExecute, buf)?)
    }
}

impl<Provider> Stage<Provider> for MerkleStage
where
    Provider: DBProvider<Tx: DbTxMut>
        + TrieWriter
        + StatsReader
        + HeaderProvider
        + ChangeSetReader
        + StorageChangeSetReader
        + StorageSettingsCache
        + StageCheckpointReader
        + StageCheckpointWriter,
{
    /// Return the id of the stage
    fn id(&self) -> StageId {
        match self {
            Self::Execution { .. } => StageId::MerkleExecute,
            Self::Unwind { .. } => StageId::MerkleUnwind,
            #[cfg(any(test, feature = "test-utils"))]
            Self::Both { .. } => StageId::Other("MerkleBoth"),
        }
    }

    /// Execute the stage.
    fn execute(&mut self, provider: &Provider, input: ExecInput) -> Result<ExecOutput, StageError> {
        let (threshold, incremental_threshold) = match self {
            Self::Unwind { .. } => {
                info!(target: "sync::stages::merkle::unwind", "Stage is always skipped");
                return Ok(ExecOutput::done(StageCheckpoint::new(input.target())))
            }
            Self::Execution { rebuild_threshold, incremental_threshold } => {
                (*rebuild_threshold, *incremental_threshold)
            }
            #[cfg(any(test, feature = "test-utils"))]
            Self::Both { rebuild_threshold, incremental_threshold } => {
                (*rebuild_threshold, *incremental_threshold)
            }
        };
        let incremental_threshold = incremental_threshold.max(1);

        let range = input.next_block_range();
        let (from_block, to_block) = range.clone().into_inner();
        let current_block_number = input.checkpoint().block_number;

        let target_block = provider
            .header_by_number(to_block)?
            .ok_or_else(|| ProviderError::HeaderNotFound(to_block.into()))?;
        let target_block_root = target_block.state_root();

        // A rebuild clears the trie tables before it starts, so if one was interrupted, finish it:
        // updating the partial trie incrementally would recompute every missing node in memory.
        let mut checkpoint = self.get_execution_checkpoint(provider)?;

        let (trie_root, entities_checkpoint) = if range.is_empty() {
            (target_block_root, input.checkpoint().entities_stage_checkpoint().unwrap_or_default())
        } else if to_block - from_block > threshold || from_block == 1 || checkpoint.is_some() {
            // if there are more blocks than threshold it is faster to rebuild the trie
            let mut entities_checkpoint = if let Some(checkpoint) =
                checkpoint.as_ref().filter(|c| c.target_block == to_block)
            {
                debug!(
                    target: "sync::stages::merkle::exec",
                    current = ?current_block_number,
                    target = ?to_block,
                    last_account_key = ?checkpoint.last_account_key,
                    "Continuing inner merkle checkpoint"
                );

                input.checkpoint().entities_stage_checkpoint()
            } else {
                debug!(
                    target: "sync::stages::merkle::exec",
                    current = ?current_block_number,
                    target = ?to_block,
                    previous_checkpoint = ?checkpoint,
                    "Rebuilding trie"
                );
                // Reset the checkpoint and clear trie tables
                checkpoint = None;
                self.save_execution_checkpoint(provider, None)?;
                provider.tx_ref().clear::<tables::AccountsTrie>()?;
                provider.tx_ref().clear::<tables::StoragesTrie>()?;

                None
            }
            .unwrap_or(EntitiesCheckpoint {
                processed: 0,
                total: (provider.count_entries::<tables::HashedAccounts>()? +
                    provider.count_entries::<tables::HashedStorages>()?)
                    as u64,
            });

            let tx = provider.tx_ref();
            let progress = reth_trie_db::with_adapter!(provider, |A| {
                DbStateRoot::<_, A>::from_tx(tx)
                    .with_intermediate_state(checkpoint.map(IntermediateStateRootState::from))
                    .root_with_progress()
            })
            .map_err(|e| {
                error!(target: "sync::stages::merkle", %e, ?current_block_number, ?to_block, "State root with progress failed! {INVALID_STATE_ROOT_ERROR_MESSAGE}");
                StageError::Fatal(Box::new(e))
            })?;
            match progress {
                StateRootProgress::Progress(state, hashed_entries_walked, updates) => {
                    provider.write_trie_updates(updates)?;

                    let mut checkpoint = MerkleCheckpoint::new(
                        to_block,
                        state.account_root_state.last_hashed_key,
                        state
                            .account_root_state
                            .walker_stack
                            .into_iter()
                            .map(StoredSubNode::from)
                            .collect(),
                        state.account_root_state.hash_builder.into(),
                    );

                    // Save storage root state if present
                    if let Some(storage_state) = state.storage_root_state {
                        checkpoint.storage_root_checkpoint =
                            Some(StorageRootMerkleCheckpoint::new(
                                storage_state.state.last_hashed_key,
                                storage_state
                                    .state
                                    .walker_stack
                                    .into_iter()
                                    .map(StoredSubNode::from)
                                    .collect(),
                                storage_state.state.hash_builder.into(),
                                storage_state.account,
                            ));
                    }
                    self.save_execution_checkpoint(provider, Some(checkpoint))?;

                    entities_checkpoint.processed += hashed_entries_walked as u64;

                    return Ok(ExecOutput::in_progress(
                        input.checkpoint().with_entities_stage_checkpoint(entities_checkpoint),
                    ))
                }
                StateRootProgress::Complete(root, hashed_entries_walked, updates) => {
                    provider.write_trie_updates(updates)?;

                    entities_checkpoint.processed += hashed_entries_walked as u64;

                    (root, entities_checkpoint)
                }
            }
        } else {
            debug!(target: "sync::stages::merkle::exec", current = ?current_block_number, target = ?to_block, "Updating trie in chunks");
            // One chunk per call: the pipeline commits after each call, so the uncommitted trie
            // updates stay bounded by one chunk and a restart resumes after the last committed one.
            //
            // Hashed state is already at the pipeline target, so a chunk's root is not the root of
            // its last block and only the final chunk can be validated. The block checkpoint stays
            // at the last validated block, which keeps it a safe unwind target, and the execution
            // checkpoint records the last committed chunk. The committed trie holds target values
            // for every key changed up to that chunk; the remaining range's prefix sets cover every
            // key that changes after it, so resuming forward, even toward a newer target, still
            // ends at the right root. `MerkleUnwind` clears the progress when it rewrites the trie.
            let chunk_from = input
                .checkpoint()
                .execution_stage_checkpoint()
                .filter(|progress| {
                    progress.block_range.from == from_block && progress.block_range.to < to_block
                })
                .map_or(from_block, |progress| progress.block_range.to + 1);
            let chunk_to = to_block.min(chunk_from.saturating_add(incremental_threshold - 1));
            debug!(
                target: "sync::stages::merkle::exec",
                current = ?current_block_number,
                target = ?to_block,
                incremental_threshold,
                chunk_range = ?(chunk_from..=chunk_to),
                "Processing chunk"
            );
            let (root, updates) = reth_trie_db::with_adapter!(provider, |A| {
                DbStateRoot::<_, A>::incremental_root_with_updates(provider, chunk_from..=chunk_to)
            })
            .map_err(|e| {
                error!(target: "sync::stages::merkle", %e, ?current_block_number, ?to_block, "Incremental state root failed! {INVALID_STATE_ROOT_ERROR_MESSAGE}");
                StageError::Fatal(Box::new(e))
            })?;
            provider.write_trie_updates(updates)?;

            if chunk_to < to_block {
                return Ok(ExecOutput::in_progress(
                    input.checkpoint().with_execution_stage_checkpoint(ExecutionCheckpoint {
                        block_range: CheckpointBlockRange { from: from_block, to: chunk_to },
                        progress: EntitiesCheckpoint {
                            processed: chunk_to - current_block_number,
                            total: to_block - current_block_number,
                        },
                    }),
                ))
            }

            let total_hashed_entries = (provider.count_entries::<tables::HashedAccounts>()? +
                provider.count_entries::<tables::HashedStorages>()?)
                as u64;

            let entities_checkpoint = EntitiesCheckpoint {
                // This is fine because `range` doesn't have an upper bound, so in this `else`
                // branch we're just hashing all remaining accounts and storage slots we have in the
                // database.
                processed: total_hashed_entries,
                total: total_hashed_entries,
            };

            // Save the checkpoint
            (root, entities_checkpoint)
        };

        // Reset the checkpoint
        self.save_execution_checkpoint(provider, None)?;

        validate_state_root(trie_root, SealedHeader::seal_slow(target_block), to_block)?;

        Ok(ExecOutput::done(
            StageCheckpoint::new(to_block).with_entities_stage_checkpoint(entities_checkpoint),
        ))
    }

    /// Unwind the stage.
    fn unwind(
        &mut self,
        provider: &Provider,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        let tx = provider.tx_ref();
        if matches!(self, Self::Execution { .. }) {
            info!(target: "sync::stages::merkle::unwind", "Stage is always skipped");
            return Ok(UnwindOutput { checkpoint: StageCheckpoint::new(input.unwind_to) })
        }
        // `MerkleExecute` is unwound first, so its block checkpoint is `unwind_to` unless it had
        // not validated that block yet. Its committed chunks only cover keys changed up to their
        // last block; keys changed after its checkpoint can still have stale nodes, so they must
        // be recomputed too. The recomputed trie no longer matches its chunk progress, so clear it.
        let merkle_execute = provider.get_stage_checkpoint(StageId::MerkleExecute)?;
        if let Some(checkpoint) =
            merkle_execute.filter(|checkpoint| checkpoint.execution_stage_checkpoint().is_some())
        {
            provider.save_stage_checkpoint(
                StageId::MerkleExecute,
                StageCheckpoint::new(checkpoint.block_number),
            )?;
        }
        // A rebuild in progress cleared the trie and writes target values, so it needs no wider
        // range, and widening it to a rebuild's range start can load the whole chain's changesets.
        let range = if self.get_execution_checkpoint(provider)?.is_some() {
            input.unwind_block_range()
        } else {
            merkle_execute
                .map_or(input.unwind_to, |checkpoint| checkpoint.block_number)
                .min(input.unwind_to) +
                1..=input.checkpoint.block_number
        };
        let walk_all_changed_branch_children = match self {
            Self::Unwind { walk_all_changed_branch_children } => *walk_all_changed_branch_children,
            #[cfg(any(test, feature = "test-utils"))]
            Self::Both { .. } => false,
            Self::Execution { .. } => unreachable!(),
        };

        let mut entities_checkpoint =
            input.checkpoint.entities_stage_checkpoint().unwrap_or(EntitiesCheckpoint {
                processed: 0,
                total: (tx.entries::<tables::HashedAccounts>()? +
                    tx.entries::<tables::HashedStorages>()?) as u64,
            });

        if input.unwind_to == 0 {
            tx.clear::<tables::AccountsTrie>()?;
            tx.clear::<tables::StoragesTrie>()?;

            entities_checkpoint.processed = 0;

            return Ok(UnwindOutput {
                checkpoint: StageCheckpoint::new(input.unwind_to)
                    .with_entities_stage_checkpoint(entities_checkpoint),
            })
        }

        // Unwind trie only if there are transitions
        if range.is_empty() {
            info!(target: "sync::stages::merkle::unwind", "Nothing to unwind");
        } else {
            let (block_root, updates) = reth_trie_db::with_adapter!(provider, |A| {
                DbStateRoot::<_, A>::incremental_root_calculator(provider, range).and_then(
                    |calculator| {
                        calculator
                            .with_walk_all_changed_branch_children(walk_all_changed_branch_children)
                            .root_with_updates()
                    },
                )
            })
            .map_err(|e| StageError::Fatal(Box::new(e)))?;

            // Validate the calculated state root
            let target = provider
                .header_by_number(input.unwind_to)?
                .ok_or_else(|| ProviderError::HeaderNotFound(input.unwind_to.into()))?;

            validate_state_root(block_root, SealedHeader::seal_slow(target), input.unwind_to)?;

            // Validation passed, apply unwind changes to the database.
            provider.write_trie_updates(updates)?;

            // Update entities checkpoint to reflect the unwind operation
            // Since we're unwinding, we need to recalculate the total entities at the target block
            let accounts = tx.entries::<tables::HashedAccounts>()?;
            let storages = tx.entries::<tables::HashedStorages>()?;
            let total = (accounts + storages) as u64;
            entities_checkpoint.total = total;
            entities_checkpoint.processed = total;
        }

        Ok(UnwindOutput {
            checkpoint: StageCheckpoint::new(input.unwind_to)
                .with_entities_stage_checkpoint(entities_checkpoint),
        })
    }
}

/// Check that the computed state root matches the root in the expected header.
#[inline]
fn validate_state_root<H: BlockHeader + Sealable + Debug>(
    got: B256,
    expected: SealedHeader<H>,
    target_block: BlockNumber,
) -> Result<(), StageError> {
    if got == expected.state_root() {
        Ok(())
    } else {
        error!(target: "sync::stages::merkle", ?target_block, ?got, ?expected, "Failed to verify block state root! {INVALID_STATE_ROOT_ERROR_MESSAGE}");
        Err(StageError::Block {
            error: BlockErrorKind::Validation(ConsensusError::BodyStateRootDiff(
                GotExpected { got, expected: expected.state_root() }.into(),
            )),
            block: Box::new(expected.block_with_parent()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{
        stage_test_suite_ext, ExecuteStageTestRunner, StageTestRunner, StorageKind,
        TestRunnerError, TestStageDB, UnwindStageTestRunner,
    };
    use alloy_primitives::{keccak256, Address, U256};
    use assert_matches::assert_matches;
    use reth_db_api::cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO};
    use reth_primitives_traits::{Account, SealedBlock, StorageEntry};
    use reth_provider::{
        providers::StaticFileWriter, BlockNumReader, ProviderResult, StaticFileProviderFactory,
    };
    use reth_stages_api::StageUnitCheckpoint;
    use reth_static_file_types::StaticFileSegment;
    use reth_testing_utils::generators::{
        self, random_block, random_block_range, random_changeset_range,
        random_contract_account_range, BlockParams, BlockRangeParams,
    };
    use reth_trie::{
        test_utils::{state_root, state_root_prehashed},
        trie_cursor::{TrieCursor, TrieCursorFactory},
        updates::{StorageTrieUpdates, TrieUpdates},
        BranchNodeCompact, Nibbles,
    };
    use reth_trie_db::DatabaseTrieCursorFactory;
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
            target: Some(previous_stage),
            checkpoint: Some(StageCheckpoint::new(stage_progress)),
        };

        runner.seed_execution(input).expect("failed to seed execution");

        let rx = runner.execute(input);

        // Assert the successful result
        let result = rx.await.unwrap();
        assert_matches!(
            result,
            Ok(ExecOutput {
                checkpoint: StageCheckpoint {
                    block_number,
                    stage_checkpoint: Some(StageUnitCheckpoint::Entities(EntitiesCheckpoint {
                        processed,
                        total
                    }))
                },
                done: true
            }) if block_number == previous_stage && processed == total &&
                total == (
                    runner.db.count_entries::<tables::HashedAccounts>().unwrap() +
                    runner.db.count_entries::<tables::HashedStorages>().unwrap()
                ) as u64
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
            target: Some(previous_stage),
            checkpoint: Some(StageCheckpoint::new(stage_progress)),
        };

        runner.seed_execution(input).expect("failed to seed execution");

        let rx = runner.execute(input);

        // Assert the successful result
        let result = rx.await.unwrap();
        assert_matches!(
            result,
            Ok(ExecOutput {
                checkpoint: StageCheckpoint {
                    block_number,
                    stage_checkpoint: Some(StageUnitCheckpoint::Entities(EntitiesCheckpoint {
                        processed,
                        total
                    }))
                },
                done: true
            }) if block_number == previous_stage && processed == total &&
                total == (
                    runner.db.count_entries::<tables::HashedAccounts>().unwrap() +
                    runner.db.count_entries::<tables::HashedStorages>().unwrap()
                ) as u64
        );

        // Validate the stage execution
        assert!(runner.validate_execution(input, result.ok()).is_ok(), "execution validation");
    }

    #[tokio::test]
    async fn execute_chunked_merkle() {
        for incremental_threshold in [0, 10, u64::MAX] {
            execute_chunked_merkle_with(incremental_threshold).await;
        }
    }

    async fn execute_chunked_merkle_with(incremental_threshold: u64) {
        let (previous_stage, stage_progress) = (200, 100);
        let clean_threshold = 100;

        // Set up the runner
        let mut runner =
            MerkleTestRunner { db: TestStageDB::default(), clean_threshold, incremental_threshold };

        let mut input = ExecInput {
            target: Some(previous_stage),
            checkpoint: Some(StageCheckpoint::new(stage_progress)),
        };

        runner.seed_execution(input).expect("failed to seed execution");

        // Each call commits one chunk and records it, keeping the block checkpoint at the last
        // validated block until the target is reached.
        let mut chunk_ends = Vec::new();
        let result = loop {
            let result = runner.execute(input).await.unwrap();
            match result {
                Ok(ExecOutput { checkpoint, done: false }) => {
                    assert_eq!(checkpoint.block_number, stage_progress);
                    chunk_ends.push(last_committed_chunk(checkpoint).unwrap());
                    input.checkpoint = Some(checkpoint);
                }
                result => break result,
            }
        };
        assert_eq!(
            chunk_ends,
            (stage_progress.saturating_add(incremental_threshold.max(1))..previous_stage)
                .step_by(incremental_threshold.max(1) as usize)
                .collect::<Vec<_>>()
        );

        assert_matches!(
            result,
            Ok(ExecOutput {
                checkpoint: StageCheckpoint {
                    block_number,
                    stage_checkpoint: Some(StageUnitCheckpoint::Entities(EntitiesCheckpoint {
                        processed,
                        total
                    }))
                },
                done: true
            }) if block_number == previous_stage && processed == total &&
                total == (
                    runner.db.count_entries::<tables::HashedAccounts>().unwrap() +
                    runner.db.count_entries::<tables::HashedStorages>().unwrap()
                ) as u64
        );

        // Validate the stage execution
        let provider = runner.db.factory.provider().unwrap();
        let header = provider.header_by_number(previous_stage).unwrap().unwrap();
        let expected_root = header.state_root;

        let actual_root = runner
            .db
            .query_with_provider(|provider| {
                Ok(reth_trie_db::with_adapter!(provider, |A| {
                    DbStateRoot::<_, A>::incremental_root_with_updates(
                        &provider,
                        stage_progress + 1..=previous_stage,
                    )
                }))
            })
            .unwrap();

        assert_eq!(
            actual_root.unwrap().0,
            expected_root,
            "State root mismatch after chunked processing"
        );
    }

    /// A crash before a chunk commits loses only that chunk: retrying the same input finishes, and
    /// the stage validates the final root against the target header.
    #[tokio::test]
    async fn incremental_merkle_retries_uncommitted_chunk() {
        let (runner, mut input) = start_round().await;

        for _ in 0..2 {
            let output = runner.execute(input).await.unwrap().unwrap();
            input.checkpoint = Some(output.checkpoint);
        }
        let provider = runner.db.factory.provider_rw().unwrap();
        let uncommitted = runner.stage().execute(&*provider, input).unwrap();
        assert_eq!(last_committed_chunk(uncommitted.checkpoint), Some(ROUND_START + 30));
        drop(provider);

        let output = execute_until_done(&runner, input).await.unwrap();
        assert_eq!(output.checkpoint.block_number, ROUND_TARGET);
    }

    /// A round interrupted after some chunks resumes from its checkpoint when the next round
    /// targets a later block, and the stage validates the root at that block.
    #[tokio::test]
    async fn incremental_merkle_resumes_toward_newer_target() {
        let (runner, mut input) = start_round().await;

        // The interrupted round targeted block 150, so hashing had only reached that block.
        let target_hashed_state = hashed_state(&runner.db);
        runner
            .before_unwind(UnwindInput {
                checkpoint: StageCheckpoint::new(ROUND_TARGET),
                unwind_to: 150,
                bad_block: None,
            })
            .unwrap();
        input.target = Some(150);
        for _ in 0..3 {
            let output = runner.execute(input).await.unwrap().unwrap();
            assert!(!output.done);
            input.checkpoint = Some(output.checkpoint);
        }
        assert_eq!(last_committed_chunk(input.checkpoint()), Some(ROUND_START + 30));

        // The next round's hashing reaches block 200 before `MerkleExecute` resumes.
        restore_hashed_state(&runner.db, target_hashed_state);
        input.target = Some(ROUND_TARGET);
        let output = execute_until_done(&runner, input).await.unwrap();
        assert_eq!(output.checkpoint.block_number, ROUND_TARGET);
    }

    /// A wrong root at the end of a chunked range fails the stage with its block checkpoint still
    /// at the last validated block, so the pipeline unwinds to a block whose root holds.
    #[tokio::test]
    async fn final_root_error_keeps_checkpoint_at_last_validated_block() {
        let (runner, mut input) = start_round().await;
        set_header_state_root(&runner.db, ROUND_TARGET, B256::repeat_byte(0x42));

        // Like the pipeline, commit each successful call and drop the transaction of a failed one.
        let error = loop {
            let provider = runner.db.factory.provider_rw().unwrap();
            match runner.stage().execute(&*provider, input) {
                Ok(output) => {
                    provider.commit().unwrap();
                    assert!(!output.done);
                    assert_eq!(output.checkpoint.block_number, ROUND_START);
                    input.checkpoint = Some(output.checkpoint);
                }
                Err(error) => break error,
            }
        };
        assert_matches!(error, StageError::Block { .. });
        assert_eq!(last_committed_chunk(input.checkpoint()), Some(ROUND_TARGET - 10));

        // On a block error the pipeline keeps the previous checkpoint and unwinds to its block.
        let provider = runner.db.factory.provider_rw().unwrap();
        provider.save_stage_checkpoint(StageId::MerkleExecute, input.checkpoint()).unwrap();
        provider.commit().unwrap();
        let unwind = UnwindInput {
            checkpoint: StageCheckpoint::new(ROUND_TARGET),
            unwind_to: input.checkpoint().block_number,
            bad_block: None,
        };
        runner.before_unwind(unwind).unwrap();
        let output = runner.unwind(unwind).await.unwrap();
        assert_eq!(output.checkpoint.block_number, ROUND_START);
    }

    /// A short range after an interrupted rebuild finishes the rebuild instead of updating the
    /// partial trie incrementally.
    #[tokio::test]
    async fn interrupted_rebuild_is_finished_on_short_range() {
        let (target, round_start) = (110, 100);
        let mut runner = MerkleTestRunner {
            db: TestStageDB::default(),
            clean_threshold: 100,
            incremental_threshold: 10,
        };
        let input =
            ExecInput { target: Some(target), checkpoint: Some(StageCheckpoint::new(round_start)) };
        runner.seed_execution(input).expect("failed to seed execution");

        // The interrupted rebuild left its inner checkpoint and a storage trie node for an account
        // that does not exist. Incremental updates never visit that storage trie; a rebuild
        // clears it.
        let stray_account = B256::repeat_byte(0xab);
        let stray_path = Nibbles::from_nibbles([0xf, 0xf]);
        let provider = runner.db.factory.provider_rw().unwrap();
        let mut storage_updates = StorageTrieUpdates::default();
        storage_updates.storage_nodes.insert(
            stray_path,
            BranchNodeCompact::new(
                0b11,
                0,
                0b11,
                vec![B256::repeat_byte(1), B256::repeat_byte(2)],
                None,
            ),
        );
        let mut updates = TrieUpdates::default();
        updates.storage_tries.insert(stray_account, storage_updates);
        provider.write_trie_updates(updates).unwrap();
        runner
            .stage()
            .save_execution_checkpoint(
                &*provider,
                Some(MerkleCheckpoint::new(target + 1, B256::ZERO, Vec::new(), Default::default())),
            )
            .unwrap();
        provider.commit().unwrap();

        let output = execute_until_done(&runner, input).await.unwrap();
        assert_eq!(output.checkpoint.block_number, target);

        let provider = runner.db.factory.provider().unwrap();
        assert!(runner.stage().get_execution_checkpoint(&provider).unwrap().is_none());
        let stray = reth_trie_db::with_adapter!(provider, |A| {
            DatabaseTrieCursorFactory::<_, A>::new(provider.tx_ref())
                .storage_trie_cursor(stray_account)
                .unwrap()
                .seek_exact(stray_path)
                .unwrap()
        });
        assert!(stray.is_none(), "the rebuild clears the partial trie");
    }

    #[tokio::test]
    async fn unwind_before_last_committed_chunk() {
        unwind_after_partial_incremental_merkle(105).await;
    }

    #[tokio::test]
    async fn unwind_past_last_committed_chunk() {
        unwind_after_partial_incremental_merkle(190).await;
    }

    /// Unwinds to `unwind_to` after `MerkleExecute` committed one chunk, through block 110, of the
    /// round from [`start_round`], then runs the next round to the target.
    async fn unwind_after_partial_incremental_merkle(unwind_to: BlockNumber) {
        let (runner, input) = start_round().await;
        let target_hashed_state = hashed_state(&runner.db);
        let output = runner.execute(input).await.unwrap().unwrap();
        assert_eq!(output.checkpoint.block_number, ROUND_START);
        assert_eq!(last_committed_chunk(output.checkpoint), Some(ROUND_START + 10));

        // The pipeline saves each call's checkpoint. It then unwinds `MerkleExecute` first, which
        // leaves it alone: its block checkpoint, the round start, is not above `unwind_to`.
        let provider = runner.db.factory.provider_rw().unwrap();
        provider.save_stage_checkpoint(StageId::MerkleExecute, output.checkpoint).unwrap();
        provider.commit().unwrap();

        let unwind = UnwindInput {
            checkpoint: StageCheckpoint::new(ROUND_TARGET),
            unwind_to,
            bad_block: None,
        };
        runner.before_unwind(unwind).unwrap();
        set_header_state_root(&runner.db, unwind_to, hashed_state_root(&runner.db).unwrap());

        // The stage validates the recomputed root against the header of `unwind_to`.
        let output = runner.unwind(unwind).await.unwrap();
        assert_eq!(output.checkpoint.block_number, unwind_to);
        let merkle_execute = runner
            .db
            .factory
            .provider()
            .unwrap()
            .get_stage_checkpoint(StageId::MerkleExecute)
            .unwrap();
        assert_eq!(merkle_execute, Some(StageCheckpoint::new(ROUND_START)));

        // The next round hashes up to the target again and resumes from `MerkleExecute`'s
        // checkpoint; the stage validates the root at the target.
        restore_hashed_state(&runner.db, target_hashed_state);
        let output = execute_until_done(
            &runner,
            ExecInput { target: Some(ROUND_TARGET), checkpoint: merkle_execute },
        )
        .await
        .unwrap();
        assert_eq!(output.checkpoint.block_number, ROUND_TARGET);
    }

    /// Returns the last block of the chunks an in-progress incremental range has committed.
    fn last_committed_chunk(checkpoint: StageCheckpoint) -> Option<BlockNumber> {
        checkpoint.execution_stage_checkpoint().map(|progress| progress.block_range.to)
    }

    const ROUND_START: BlockNumber = 100;
    const ROUND_TARGET: BlockNumber = 200;
    /// Block at which every tenth extra account in [`start_round`] changes, and nothing else.
    const LATE_CHANGE_BLOCK: BlockNumber = 151;

    /// Sets up a pipeline round as `MerkleExecute` finds it: the trie built at block 100, hashed
    /// state at block 200, and `MerkleUnwind` already at 200. The state is wide enough that the
    /// trie stores branch hashes below the root, so trie nodes the stage fails to update stay
    /// stale and break the next validated root.
    // `Account` is only `Copy` without the `account-ext` feature.
    #[allow(clippy::clone_on_copy)]
    async fn start_round() -> (MerkleTestRunner, ExecInput) {
        let mut runner = MerkleTestRunner {
            db: TestStageDB::default(),
            clean_threshold: 1000,
            incremental_threshold: 10,
        };
        let input = ExecInput {
            target: Some(ROUND_TARGET),
            checkpoint: Some(StageCheckpoint::new(ROUND_START)),
        };
        runner.seed_execution(input).expect("failed to seed execution");

        let extra_accounts = (1..=2000u64)
            .map(|i| {
                let address = Address::from_word(B256::from(U256::from(i)));
                (address, Account { nonce: 1, balance: U256::from(i), ..Default::default() })
            })
            .collect::<Vec<_>>();
        runner
            .db
            .insert_accounts_and_storages(
                extra_accounts
                    .iter()
                    .map(|(address, account)| (*address, (account.clone(), std::iter::empty()))),
            )
            .unwrap();
        let late_changes = extra_accounts.iter().step_by(10);
        runner
            .db
            .insert_changesets(
                [late_changes
                    .clone()
                    .map(|(address, account)| (*address, account.clone(), Vec::new()))
                    .collect::<Vec<_>>()],
                Some(LATE_CHANGE_BLOCK),
            )
            .unwrap();
        runner
            .db
            .insert_accounts_and_storages(late_changes.map(|(address, account)| {
                let account =
                    Account { balance: account.balance + U256::from(1), ..account.clone() };
                (*address, (account, std::iter::empty()))
            }))
            .unwrap();

        // Give the start and target headers the roots of the widened state.
        set_header_state_root(&runner.db, ROUND_TARGET, hashed_state_root(&runner.db).unwrap());
        let target_hashed_state = hashed_state(&runner.db);
        let to_round_start = UnwindInput {
            checkpoint: StageCheckpoint::new(ROUND_TARGET),
            unwind_to: ROUND_START,
            bad_block: None,
        };
        runner.before_unwind(to_round_start).unwrap();
        set_header_state_root(&runner.db, ROUND_START, hashed_state_root(&runner.db).unwrap());

        // Build the trie at the round start from scratch, then move hashing to the target.
        runner.clean_threshold = 0;
        execute_until_done(
            &runner,
            ExecInput { target: Some(ROUND_START), checkpoint: Some(StageCheckpoint::new(0)) },
        )
        .await
        .unwrap();
        runner.clean_threshold = 1000;
        restore_hashed_state(&runner.db, target_hashed_state);

        (runner, input)
    }

    /// Drives the stage to completion one committed call at a time, as the pipeline does.
    async fn execute_until_done(
        runner: &MerkleTestRunner,
        mut input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        loop {
            match runner.execute(input).await.unwrap()? {
                ExecOutput { checkpoint, done: false } => input.checkpoint = Some(checkpoint),
                output => return Ok(output),
            }
        }
    }

    type HashedState = (Vec<(B256, Account)>, Vec<(B256, StorageEntry)>);

    fn hashed_state(db: &TestStageDB) -> HashedState {
        db.query(|tx| {
            let accounts = tx
                .cursor_read::<tables::HashedAccounts>()?
                .walk(None)?
                .collect::<Result<Vec<_>, _>>()?;
            let storages = tx
                .cursor_dup_read::<tables::HashedStorages>()?
                .walk(None)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok((accounts, storages))
        })
        .unwrap()
    }

    fn restore_hashed_state(db: &TestStageDB, (accounts, storages): HashedState) {
        db.commit(|tx| {
            tx.clear::<tables::HashedAccounts>()?;
            tx.clear::<tables::HashedStorages>()?;
            for (hashed_address, account) in accounts {
                tx.put::<tables::HashedAccounts>(hashed_address, account)?;
            }
            for (hashed_address, entry) in storages {
                tx.put::<tables::HashedStorages>(hashed_address, entry)?;
            }
            Ok(())
        })
        .unwrap();
    }

    /// Computes the state root of the hashed state tables from scratch.
    fn hashed_state_root(db: &TestStageDB) -> ProviderResult<B256> {
        db.query(|tx| {
            let mut accounts = BTreeMap::default();
            let mut accounts_cursor = tx.cursor_read::<tables::HashedAccounts>()?;
            let mut storage_cursor = tx.cursor_dup_read::<tables::HashedStorages>()?;
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
                    .filter(|v| !v.value.is_zero())
                    .map(|v| (v.key, v.value))
                    .collect::<Vec<_>>();
                accounts.insert(key, (account, storage));
            }

            Ok(state_root_prehashed(accounts))
        })
    }

    /// Rewrites the header of `block` with `state_root`, re-appending the headers after it.
    fn set_header_state_root(db: &TestStageDB, block: BlockNumber, state_root: B256) {
        let provider = db.factory.provider().unwrap();
        let last = provider.last_block_number().unwrap();
        let headers = (block..=last)
            .map(|number| provider.header_by_number(number).unwrap().unwrap())
            .collect::<Vec<_>>();
        drop(provider);

        let static_file_provider = db.factory.static_file_provider();
        let mut writer = static_file_provider.latest_writer(StaticFileSegment::Headers).unwrap();
        writer.prune_headers(headers.len() as u64).unwrap();
        writer.commit().unwrap();
        for (i, mut header) in headers.into_iter().enumerate() {
            if i == 0 {
                header.state_root = state_root;
            }
            let hash = header.hash_slow();
            writer.append_header(&header, &hash).unwrap();
        }
        writer.commit().unwrap();
    }

    struct MerkleTestRunner {
        db: TestStageDB,
        clean_threshold: u64,
        incremental_threshold: u64,
    }

    impl Default for MerkleTestRunner {
        fn default() -> Self {
            Self {
                db: TestStageDB::default(),
                clean_threshold: 10000,
                incremental_threshold: 10000,
            }
        }
    }

    impl StageTestRunner for MerkleTestRunner {
        type S = MerkleStage;

        fn db(&self) -> &TestStageDB {
            &self.db
        }

        fn stage(&self) -> Self::S {
            Self::S::Both {
                rebuild_threshold: self.clean_threshold,
                incremental_threshold: self.incremental_threshold,
            }
        }
    }

    impl ExecuteStageTestRunner for MerkleTestRunner {
        type Seed = Vec<SealedBlock<reth_ethereum_primitives::Block>>;

        #[allow(clippy::clone_on_copy)]
        fn seed_execution(&mut self, input: ExecInput) -> Result<Self::Seed, TestRunnerError> {
            let stage_progress = input.checkpoint().block_number;
            let start = stage_progress + 1;
            let end = input.target();
            let mut rng = generators::rng();

            let mut preblocks = vec![];
            if stage_progress > 0 {
                preblocks.append(&mut random_block_range(
                    &mut rng,
                    0..=stage_progress - 1,
                    BlockRangeParams {
                        parent: Some(B256::ZERO),
                        tx_count: 0..1,
                        ..Default::default()
                    },
                ));
                self.db.insert_blocks(preblocks.iter(), StorageKind::Static)?;
            }

            let num_of_accounts = 31;
            let accounts = random_contract_account_range(&mut rng, &mut (0..num_of_accounts))
                .into_iter()
                .collect::<BTreeMap<_, _>>();

            self.db.insert_accounts_and_storages(
                accounts.iter().map(|(addr, acc)| (*addr, (acc.clone(), std::iter::empty()))),
            )?;

            let (header, body) = random_block(
                &mut rng,
                stage_progress,
                BlockParams { parent: preblocks.last().map(|b| b.hash()), ..Default::default() },
            )
            .split_sealed_header_body();
            let mut header = header.unseal();

            header.state_root = state_root(
                accounts
                    .clone()
                    .into_iter()
                    .map(|(address, account)| (address, (account, std::iter::empty()))),
            );
            let sealed_head = SealedBlock::<reth_ethereum_primitives::Block>::from_sealed_parts(
                SealedHeader::seal_slow(header),
                body,
            );

            let head_hash = sealed_head.hash();
            let mut blocks = vec![sealed_head];
            blocks.extend(random_block_range(
                &mut rng,
                start..=end,
                BlockRangeParams { parent: Some(head_hash), tx_count: 0..3, ..Default::default() },
            ));
            let last_block = blocks.last().cloned().unwrap();
            self.db.insert_blocks(blocks.iter(), StorageKind::Static)?;

            let (transitions, final_state) = random_changeset_range(
                &mut rng,
                // The head block's state is already in `accounts`; changesets start at `start`.
                blocks.iter().skip(1),
                accounts.into_iter().map(|(addr, acc)| (addr, (acc, Vec::new()))),
                0..3,
                0..256,
            );
            // add block changeset from block 1.
            self.db.insert_changesets(transitions, Some(start))?;
            self.db.insert_accounts_and_storages(final_state)?;

            let root = hashed_state_root(&self.db)?;

            let static_file_provider = self.db.factory.static_file_provider();
            let mut writer =
                static_file_provider.latest_writer(StaticFileSegment::Headers).unwrap();
            let mut last_header = last_block.clone_sealed_header();
            last_header.set_state_root(root);

            let hash = last_header.hash_slow();
            writer.prune_headers(1).unwrap();
            writer.commit().unwrap();
            writer.append_header(&last_header, &hash).unwrap();
            writer.commit().unwrap();

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
        fn validate_unwind(&self, _input: UnwindInput) -> Result<(), TestRunnerError> {
            // The unwind is validated within the stage
            Ok(())
        }

        fn before_unwind(&self, input: UnwindInput) -> Result<(), TestRunnerError> {
            let target_block = input.unwind_to + 1;

            self.db
                .commit(|tx| {
                    let mut storage_changesets_cursor =
                        tx.cursor_dup_read::<tables::StorageChangeSets>().unwrap();
                    let mut storage_cursor =
                        tx.cursor_dup_write::<tables::HashedStorages>().unwrap();

                    let mut tree: BTreeMap<B256, BTreeMap<B256, U256>> = BTreeMap::new();

                    let mut rev_changeset_walker =
                        storage_changesets_cursor.walk_back(None).unwrap();
                    while let Some((bn_address, entry)) =
                        rev_changeset_walker.next().transpose().unwrap()
                    {
                        if bn_address.block_number() < target_block {
                            break
                        }

                        tree.entry(keccak256(bn_address.address()))
                            .or_default()
                            .insert(keccak256(entry.key), entry.value);
                    }
                    for (hashed_address, storage) in tree {
                        for (hashed_slot, value) in storage {
                            let storage_entry = storage_cursor
                                .seek_by_key_subkey(hashed_address, hashed_slot)
                                .unwrap();
                            if storage_entry.is_some_and(|v| v.key == hashed_slot) {
                                storage_cursor.delete_current().unwrap();
                            }

                            if !value.is_zero() {
                                let storage_entry = StorageEntry { key: hashed_slot, value };
                                storage_cursor.upsert(hashed_address, &storage_entry).unwrap();
                            }
                        }
                    }

                    let mut changeset_cursor =
                        tx.cursor_dup_write::<tables::AccountChangeSets>().unwrap();
                    let mut rev_changeset_walker = changeset_cursor.walk_back(None).unwrap();

                    while let Some((block_number, account_before_tx)) =
                        rev_changeset_walker.next().transpose().unwrap()
                    {
                        if block_number < target_block {
                            break
                        }

                        if let Some(acc) = account_before_tx.info {
                            tx.put::<tables::HashedAccounts>(
                                keccak256(account_before_tx.address),
                                acc,
                            )
                            .unwrap();
                        } else {
                            tx.delete::<tables::HashedAccounts>(
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
    }
}
