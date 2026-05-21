use alloy_consensus::{constants::KECCAK_EMPTY, BlockHeader};
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
    BlockErrorKind, EntitiesCheckpoint, ExecInput, ExecOutput, MerkleCheckpoint, Stage,
    StageCheckpoint, StageError, StageId, StorageRootMerkleCheckpoint, UnwindInput, UnwindOutput,
};
use reth_trie::{IntermediateStateRootState, StateRoot, StateRootProgress, StoredSubNode};
use reth_trie_db::DatabaseStateRoot;
use reth_trie_parallel::rebuild::{
    ParallelRebuildStateRoot, StoragePrefixPlannerConfig, StorageRootPrefetchConfig,
};

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

const PARALLEL_REBUILD_ENV: &str = "RETH_EXPERIMENTAL_PARALLEL_REBUILD";
const PARALLEL_REBUILD_PROGRESS_THRESHOLD_ENV: &str =
    "RETH_EXPERIMENTAL_PARALLEL_REBUILD_PROGRESS_THRESHOLD";
const PARALLEL_REBUILD_STORAGE_PREFIXES_ENV: &str =
    "RETH_EXPERIMENTAL_PARALLEL_REBUILD_STORAGE_PREFIXES";
const PARALLEL_REBUILD_STORAGE_PREFIX_MAX_DEPTH_ENV: &str =
    "RETH_EXPERIMENTAL_PARALLEL_REBUILD_STORAGE_PREFIX_MAX_DEPTH";
const PARALLEL_REBUILD_STORAGE_PREFIX_SAMPLE_LIMIT_ENV: &str =
    "RETH_EXPERIMENTAL_PARALLEL_REBUILD_STORAGE_PREFIX_SAMPLE_LIMIT";
const PARALLEL_REBUILD_STORAGE_PREFIX_MAX_PREFIXES_ENV: &str =
    "RETH_EXPERIMENTAL_PARALLEL_REBUILD_STORAGE_PREFIX_MAX_PREFIXES";
const PARALLEL_REBUILD_STORAGE_PREFIX_MIN_SPLIT_ENV: &str =
    "RETH_EXPERIMENTAL_PARALLEL_REBUILD_STORAGE_PREFIX_MIN_SPLIT";
const PARALLEL_REBUILD_STORAGE_PREFIX_TRIGGER_THRESHOLD_ENV: &str =
    "RETH_EXPERIMENTAL_PARALLEL_REBUILD_STORAGE_PREFIX_TRIGGER_THRESHOLD";
const PARALLEL_REBUILD_STORAGE_PREFIX_MIN_LARGE_SLOTS_ENV: &str =
    "RETH_EXPERIMENTAL_PARALLEL_REBUILD_STORAGE_PREFIX_MIN_LARGE_SLOTS";
const PARALLEL_REBUILD_INLINE_EMPTY_STORAGE_ENV: &str =
    "RETH_EXPERIMENTAL_PARALLEL_REBUILD_INLINE_EMPTY_STORAGE";
const PARALLEL_REBUILD_INLINE_STORAGE_SLOTS_ENV: &str =
    "RETH_EXPERIMENTAL_PARALLEL_REBUILD_INLINE_STORAGE_SLOTS";
const PARALLEL_REBUILD_ACCOUNT_PREFIX_MAX_DEPTH_ENV: &str =
    "RETH_EXPERIMENTAL_PARALLEL_REBUILD_ACCOUNT_PREFIX_MAX_DEPTH";
const PARALLEL_REBUILD_ACCOUNT_PREFIX_WINDOW_SIZE_ENV: &str =
    "RETH_EXPERIMENTAL_PARALLEL_REBUILD_ACCOUNT_PREFIX_WINDOW_SIZE";
const DEFAULT_PARALLEL_REBUILD_PROGRESS_THRESHOLD: u64 = 100_000;

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
    Unwind,
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
        Self::Unwind
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
    Provider::Tx: Sync,
{
    /// Return the id of the stage
    fn id(&self) -> StageId {
        match self {
            Self::Execution { .. } => StageId::MerkleExecute,
            Self::Unwind => StageId::MerkleUnwind,
            #[cfg(any(test, feature = "test-utils"))]
            Self::Both { .. } => StageId::Other("MerkleBoth"),
        }
    }

    /// Execute the stage.
    fn execute(&mut self, provider: &Provider, input: ExecInput) -> Result<ExecOutput, StageError> {
        let (threshold, incremental_threshold) = match self {
            Self::Unwind => {
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

        let range = input.next_block_range();
        let (from_block, to_block) = range.clone().into_inner();
        let current_block_number = input.checkpoint().block_number;

        let target_block = provider
            .header_by_number(to_block)?
            .ok_or_else(|| ProviderError::HeaderNotFound(to_block.into()))?;
        let target_block_root = target_block.state_root();

        let (trie_root, entities_checkpoint) = if range.is_empty() {
            (target_block_root, input.checkpoint().entities_stage_checkpoint().unwrap_or_default())
        } else if to_block - from_block > threshold || from_block == 1 {
            let mut checkpoint = self.get_execution_checkpoint(provider)?;

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
            let parallel_config = parallel_rebuild_config_from_env().filter(|_| {
                let supported = parallel_rebuild_checkpoint_supported(checkpoint.as_ref());
                if !supported {
                    debug!(
                        target: "sync::stages::merkle::exec",
                        current = ?current_block_number,
                        target = ?to_block,
                        "Falling back to serial trie rebuild because checkpoint is not supported by experimental parallel rebuild"
                    );
                }
                supported
            });
            let progress = if let Some(config) = parallel_config {
                debug!(
                    target: "sync::stages::merkle::exec",
                    current = ?current_block_number,
                    target = ?to_block,
                    progress_threshold = config.progress_threshold,
                    inline_empty_storage_roots = config.inline_empty_storage_roots,
                    inline_storage_root_slot_limit = config.inline_storage_root_slot_limit,
                    storage_prefix_planner = config.storage_prefix_planner.is_some(),
                    storage_prefix_trigger_threshold = config.storage_prefix_trigger_threshold,
                    storage_prefix_min_large_slots = config.storage_prefix_min_large_slots,
                    account_prefix_max_depth = config.account_prefix_max_depth,
                    account_prefix_window_size = config.account_prefix_window_size,
                    "Rebuilding trie with experimental account-prefix parallel rebuild"
                );

                let root_started = std::time::Instant::now();
                let previous_state = checkpoint.map(IntermediateStateRootState::from);
                let builder = ParallelRebuildStateRoot::new(
                    reth_trie_db::DatabaseHashedCursorFactory::new(tx),
                )
                .with_config(config)
                .with_intermediate_state(previous_state);
                let outcome = builder.root_with_progress_and_stats().map_err(|e| {
                    error!(target: "sync::stages::merkle", %e, ?current_block_number, ?to_block, "Parallel state root rebuild failed! {INVALID_STATE_ROOT_ERROR_MESSAGE}");
                    StageError::Fatal(Box::new(e))
                })?;
                let root_duration = root_started.elapsed();

                let (outcome_kind, hashed_entries_walked) = match &outcome.progress {
                    StateRootProgress::Progress(_, walked, _) => ("progress", *walked),
                    StateRootProgress::Complete(_, walked, _) => ("complete", *walked),
                };
                let stats = outcome.prefetch;
                debug!(
                    target: "sync::stages::merkle::exec",
                    outcome = outcome_kind,
                    hashed_entries_walked,
                    root_duration = ?root_duration,
                    ?stats,
                    "Experimental parallel rebuild root calculation stats"
                );
                outcome.progress
            } else {
                reth_trie_db::with_adapter!(provider, |A| {
                    DbStateRoot::<_, A>::from_tx(tx)
                        .with_intermediate_state(checkpoint.map(IntermediateStateRootState::from))
                        .root_with_progress()
                })
                .map_err(|e| {
                    error!(target: "sync::stages::merkle", %e, ?current_block_number, ?to_block, "State root with progress failed! {INVALID_STATE_ROOT_ERROR_MESSAGE}");
                    StageError::Fatal(Box::new(e))
                })?
            };

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
                                storage_state.account.nonce,
                                storage_state.account.balance,
                                storage_state.account.bytecode_hash.unwrap_or(KECCAK_EMPTY),
                            ));
                    }
                    self.save_execution_checkpoint(provider, Some(checkpoint))?;

                    entities_checkpoint.processed += hashed_entries_walked as u64;

                    return Ok(ExecOutput {
                        checkpoint: input
                            .checkpoint()
                            .with_entities_stage_checkpoint(entities_checkpoint),
                        done: false,
                    })
                }
                StateRootProgress::Complete(root, hashed_entries_walked, updates) => {
                    provider.write_trie_updates(updates)?;

                    entities_checkpoint.processed += hashed_entries_walked as u64;

                    (root, entities_checkpoint)
                }
            }
        } else {
            debug!(target: "sync::stages::merkle::exec", current = ?current_block_number, target = ?to_block, "Updating trie in chunks");
            let mut final_root = None;
            for start_block in range.step_by(incremental_threshold as usize) {
                let chunk_to = std::cmp::min(start_block + incremental_threshold, to_block);
                let chunk_range = start_block..=chunk_to;
                debug!(
                    target: "sync::stages::merkle::exec",
                    current = ?current_block_number,
                    target = ?to_block,
                    incremental_threshold,
                    chunk_range = ?chunk_range,
                    "Processing chunk"
                );
                let (root, updates) = reth_trie_db::with_adapter!(provider, |A| {
                    DbStateRoot::<_, A>::incremental_root_with_updates(provider, chunk_range)
                })
                .map_err(|e| {
                    error!(target: "sync::stages::merkle", %e, ?current_block_number, ?to_block, "Incremental state root failed! {INVALID_STATE_ROOT_ERROR_MESSAGE}");
                    StageError::Fatal(Box::new(e))
                })?;
                provider.write_trie_updates(updates)?;
                final_root = Some(root);
            }

            // if we had no final root, we must have not looped above, which should not be possible
            let final_root = final_root.ok_or(StageError::Fatal(
                "Incremental merkle hashing did not produce a final root".into(),
            ))?;

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
            (final_root, entities_checkpoint)
        };

        // Reset the checkpoint
        self.save_execution_checkpoint(provider, None)?;

        validate_state_root(trie_root, SealedHeader::seal_slow(target_block), to_block)?;

        Ok(ExecOutput {
            checkpoint: StageCheckpoint::new(to_block)
                .with_entities_stage_checkpoint(entities_checkpoint),
            done: true,
        })
    }

    /// Unwind the stage.
    fn unwind(
        &mut self,
        provider: &Provider,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        let tx = provider.tx_ref();
        let range = input.unwind_block_range();
        if matches!(self, Self::Execution { .. }) {
            info!(target: "sync::stages::merkle::unwind", "Stage is always skipped");
            return Ok(UnwindOutput { checkpoint: StageCheckpoint::new(input.unwind_to) })
        }

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
                DbStateRoot::<_, A>::incremental_root_with_updates(provider, range)
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

fn parallel_rebuild_config_from_env() -> Option<StorageRootPrefetchConfig> {
    if !env_enabled(PARALLEL_REBUILD_ENV) {
        return None
    }

    let mut config = StorageRootPrefetchConfig {
        progress_threshold: DEFAULT_PARALLEL_REBUILD_PROGRESS_THRESHOLD,
        ..StorageRootPrefetchConfig::default()
    };
    if let Some(progress_threshold) = env_parse(PARALLEL_REBUILD_PROGRESS_THRESHOLD_ENV) {
        config.progress_threshold = progress_threshold;
    }
    if env_enabled(PARALLEL_REBUILD_INLINE_EMPTY_STORAGE_ENV) {
        config.inline_empty_storage_roots = true;
    }
    if let Some(slot_limit) = env_parse(PARALLEL_REBUILD_INLINE_STORAGE_SLOTS_ENV) {
        config.inline_storage_root_slot_limit = slot_limit;
    }
    if let Some(max_depth) = env_parse(PARALLEL_REBUILD_ACCOUNT_PREFIX_MAX_DEPTH_ENV) {
        config.account_prefix_max_depth = max_depth;
    }
    if let Some(window_size) = env_parse(PARALLEL_REBUILD_ACCOUNT_PREFIX_WINDOW_SIZE_ENV) {
        config.account_prefix_window_size = window_size;
    }
    if env_enabled(PARALLEL_REBUILD_STORAGE_PREFIXES_ENV) {
        let mut planner = StoragePrefixPlannerConfig::default();
        if let Some(max_depth) = env_parse(PARALLEL_REBUILD_STORAGE_PREFIX_MAX_DEPTH_ENV) {
            planner.max_depth = max_depth;
        }
        if let Some(sample_limit) = env_parse(PARALLEL_REBUILD_STORAGE_PREFIX_SAMPLE_LIMIT_ENV) {
            planner.sample_limit_per_prefix = sample_limit;
        }
        if let Some(max_prefixes) = env_parse(PARALLEL_REBUILD_STORAGE_PREFIX_MAX_PREFIXES_ENV) {
            planner.max_prefixes = max_prefixes;
        }
        if let Some(min_split) = env_parse(PARALLEL_REBUILD_STORAGE_PREFIX_MIN_SPLIT_ENV) {
            planner.min_sampled_slots_to_split = min_split;
        }
        if let Some(trigger_threshold) =
            env_parse::<u64>(PARALLEL_REBUILD_STORAGE_PREFIX_TRIGGER_THRESHOLD_ENV)
        {
            config.storage_prefix_trigger_threshold = trigger_threshold.max(1);
        }
        if let Some(min_large_slots) =
            env_parse::<usize>(PARALLEL_REBUILD_STORAGE_PREFIX_MIN_LARGE_SLOTS_ENV)
        {
            config.storage_prefix_min_large_slots = min_large_slots.max(1);
        }
        config.storage_prefix_planner = Some(planner);
    }
    (config.account_prefix_max_depth > 0).then_some(config)
}

fn env_enabled(name: &str) -> bool {
    std::env::var(name).ok().is_some_and(|value| {
        matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON")
    })
}

fn env_parse<T: std::str::FromStr>(name: &str) -> Option<T> {
    std::env::var(name).ok()?.parse().ok()
}

fn parallel_rebuild_checkpoint_supported(checkpoint: Option<&MerkleCheckpoint>) -> bool {
    checkpoint.is_none_or(|checkpoint| {
        checkpoint.walker_stack.is_empty() &&
            checkpoint
                .storage_root_checkpoint
                .as_ref()
                .is_none_or(|checkpoint| checkpoint.walker_stack.is_empty())
    })
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
        providers::StaticFileWriter, DatabaseProviderFactory, StaticFileProviderFactory,
    };
    use reth_stages_api::StageUnitCheckpoint;
    use reth_static_file_types::StaticFileSegment;
    use reth_testing_utils::generators::{
        self, random_block, random_block_range, random_changeset_range,
        random_contract_account_range, BlockParams, BlockRangeParams,
    };
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
        let (previous_stage, stage_progress) = (200, 100);
        let clean_threshold = 100;
        let incremental_threshold = 10;

        // Set up the runner
        let mut runner =
            MerkleTestRunner { db: TestStageDB::default(), clean_threshold, incremental_threshold };

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

    #[test]
    fn parallel_rebuild_segmented_checkpoint_roundtrip_through_stage_db() {
        let db = TestStageDB::default();
        let stage = MerkleStage::new_execution(1, 1);
        let address = Address::repeat_byte(0x99);
        let hashed_address = keccak256(address);
        let account = Account { nonce: 1, balance: U256::from(1), bytecode_hash: None };
        let storage = segmented_stage_storage();
        let expected_root =
            state_root_prehashed(BTreeMap::from([(hashed_address, (account, storage.clone()))]));

        db.commit(|tx| {
            tx.put::<tables::HashedAccounts>(hashed_address, account)?;
            let mut cursor = tx.cursor_dup_write::<tables::HashedStorages>()?;
            for (key, value) in &storage {
                cursor.upsert(hashed_address, &StorageEntry { key: *key, value: *value })?;
            }
            Ok(())
        })
        .unwrap();

        let config = StorageRootPrefetchConfig {
            progress_threshold: 1,
            storage_prefix_planner: Some(StoragePrefixPlannerConfig {
                max_depth: 1,
                sample_limit_per_prefix: 256,
                max_prefixes: 16,
                min_sampled_slots_to_split: 256,
            }),
            storage_prefix_min_large_slots: 1,
            ..StorageRootPrefetchConfig::default()
        };

        let mut saw_segmented_storage_checkpoint = false;
        let root = run_parallel_rebuild_checkpoint_roundtrip(&db, &stage, config, |state| {
            saw_segmented_storage_checkpoint |=
                state.storage_root_state.as_ref().is_some_and(|storage_state| {
                    storage_state.state.walker_stack.is_empty() &&
                        !storage_state.state.hash_builder.key.is_empty() &&
                        storage_state.state.hash_builder.key.len() < 64
                });
        });

        assert!(saw_segmented_storage_checkpoint);
        assert_eq!(root, expected_root);
        let provider = db.factory.database_provider_rw().unwrap();
        assert!(stage.get_execution_checkpoint(&provider).unwrap().is_none());
        provider.commit().unwrap();
    }

    #[test]
    fn parallel_rebuild_account_prefix_checkpoint_roundtrip_through_stage_db() {
        let db = TestStageDB::default();
        let stage = MerkleStage::new_execution(1, 1);
        let mut expected_state = BTreeMap::new();

        db.commit(|tx| {
            let mut storage_cursor = tx.cursor_dup_write::<tables::HashedStorages>()?;
            for prefix in 0..6u8 {
                for child in 0..16u8 {
                    let hashed_address = hashed_slot_with_prefix(&[prefix, child], 0);
                    let account_index = prefix as u64 * 16 + child as u64;
                    let account = Account {
                        nonce: account_index + 1,
                        balance: U256::from(account_index + 1),
                        bytecode_hash: None,
                    };
                    let storage = vec![(
                        hashed_slot_with_prefix(&[prefix, child, 0], 0),
                        U256::from(account_index + 1),
                    )];

                    tx.put::<tables::HashedAccounts>(hashed_address, account)?;
                    for (key, value) in &storage {
                        storage_cursor
                            .upsert(hashed_address, &StorageEntry { key: *key, value: *value })?;
                    }
                    expected_state.insert(hashed_address, (account, storage));
                }
            }
            Ok(())
        })
        .unwrap();

        let expected_root = state_root_prehashed(expected_state);
        let config = StorageRootPrefetchConfig {
            progress_threshold: 1,
            account_prefix_max_depth: 2,
            account_prefix_window_size: 4,
            ..StorageRootPrefetchConfig::default()
        };

        let mut saw_account_prefix_checkpoint = false;
        let root = run_parallel_rebuild_checkpoint_roundtrip(&db, &stage, config, |state| {
            assert!(state.storage_root_state.is_none());
            assert!(state.account_root_state.walker_stack.is_empty());
            saw_account_prefix_checkpoint = true;
        });

        assert!(saw_account_prefix_checkpoint);
        assert_eq!(root, expected_root);
        let provider = db.factory.database_provider_rw().unwrap();
        assert!(stage.get_execution_checkpoint(&provider).unwrap().is_none());
        provider.commit().unwrap();
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
                accounts.iter().map(|(addr, acc)| (*addr, (*acc, std::iter::empty()))),
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
                blocks.iter(),
                accounts.into_iter().map(|(addr, acc)| (addr, (acc, Vec::new()))),
                0..3,
                0..256,
            );
            // add block changeset from block 1.
            self.db.insert_changesets(transitions, Some(start))?;
            self.db.insert_accounts_and_storages(final_state)?;

            // Calculate state root
            let root = self.db.query(|tx| {
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
            })?;

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

    fn run_parallel_rebuild_checkpoint_roundtrip(
        db: &TestStageDB,
        stage: &MerkleStage,
        config: StorageRootPrefetchConfig,
        mut inspect_progress: impl FnMut(&IntermediateStateRootState),
    ) -> B256 {
        let mut checkpoint = None;
        loop {
            let provider = db.factory.database_provider_rw().unwrap();
            let outcome = ParallelRebuildStateRoot::new(
                reth_trie_db::DatabaseHashedCursorFactory::new(provider.tx_ref()),
            )
            .with_config(config)
            .with_intermediate_state(checkpoint.map(IntermediateStateRootState::from))
            .root_with_progress_and_stats()
            .unwrap();

            match outcome.progress {
                StateRootProgress::Progress(state, _, updates) => {
                    let state = *state;
                    inspect_progress(&state);
                    provider.write_trie_updates(updates).unwrap();
                    stage
                        .save_execution_checkpoint(
                            &provider,
                            Some(merkle_checkpoint_from_state(1, state)),
                        )
                        .unwrap();
                    provider.commit().unwrap();

                    let provider = db.factory.database_provider_rw().unwrap();
                    checkpoint = stage.get_execution_checkpoint(&provider).unwrap();
                    provider.commit().unwrap();
                }
                StateRootProgress::Complete(root, _, updates) => {
                    provider.write_trie_updates(updates).unwrap();
                    stage.save_execution_checkpoint(&provider, None).unwrap();
                    provider.commit().unwrap();
                    return root
                }
            }
        }
    }

    fn merkle_checkpoint_from_state(
        target_block: BlockNumber,
        state: IntermediateStateRootState,
    ) -> MerkleCheckpoint {
        let mut checkpoint = MerkleCheckpoint::new(
            target_block,
            state.account_root_state.last_hashed_key,
            state.account_root_state.walker_stack.into_iter().map(StoredSubNode::from).collect(),
            state.account_root_state.hash_builder.into(),
        );

        if let Some(storage_state) = state.storage_root_state {
            checkpoint.storage_root_checkpoint = Some(StorageRootMerkleCheckpoint::new(
                storage_state.state.last_hashed_key,
                storage_state.state.walker_stack.into_iter().map(StoredSubNode::from).collect(),
                storage_state.state.hash_builder.into(),
                storage_state.account.nonce,
                storage_state.account.balance,
                storage_state.account.bytecode_hash.unwrap_or(KECCAK_EMPTY),
            ));
        }

        checkpoint
    }

    fn segmented_stage_storage() -> Vec<(B256, U256)> {
        (0..4)
            .flat_map(|prefix| {
                (0..16).flat_map(move |child| {
                    (0..4).map(move |leaf| {
                        (
                            hashed_slot_with_prefix(&[prefix, child, leaf], leaf as u64),
                            U256::from(1),
                        )
                    })
                })
            })
            .collect()
    }

    fn hashed_slot_with_prefix(prefix: &[u8], index: u64) -> B256 {
        let mut bytes = [0u8; 32];
        for (idx, nibble) in prefix.iter().copied().enumerate() {
            let byte = &mut bytes[idx / 2];
            if idx % 2 == 0 {
                *byte |= nibble << 4;
            } else {
                *byte |= nibble;
            }
        }
        bytes[24..].copy_from_slice(&index.to_be_bytes());
        B256::from(bytes)
    }
}
