use alloc::vec::Vec;
use alloy_consensus::transaction::Either;
use alloy_primitives::BlockNumber;
use reth_execution_types::{BlockExecutionOutput, ExecutionOutcome};
use reth_storage_errors::provider::ProviderResult;
use reth_trie_common::HashedPostStateSorted;
use revm_database::{
    states::{PlainStateReverts, StateChangeset},
    BundleState, OriginalValuesKnown,
};

/// A helper type used as input to [`StateWriter`] for writing execution outcome for one or many
/// blocks.
#[derive(Debug)]
pub enum WriteStateInput<'a, R> {
    /// A single block execution outcome.
    Single {
        /// The execution outcome.
        outcome: &'a BlockExecutionOutput<R>,
        /// Block number
        block: BlockNumber,
    },
    /// Multiple block execution outcomes.
    Multiple(&'a ExecutionOutcome<R>),
}

impl<'a, R> WriteStateInput<'a, R> {
    /// Number of blocks in the execution outcome.
    pub const fn len(&self) -> usize {
        match self {
            Self::Single { .. } => 1,
            Self::Multiple(outcome) => outcome.len(),
        }
    }

    /// Returns true if the execution outcome is empty.
    pub const fn is_empty(&self) -> bool {
        match self {
            Self::Single { outcome, .. } => outcome.result.receipts.is_empty(),
            Self::Multiple(outcome) => outcome.is_empty(),
        }
    }

    /// Number of the first block.
    pub const fn first_block(&self) -> BlockNumber {
        match self {
            Self::Single { block, .. } => *block,
            Self::Multiple(outcome) => outcome.first_block(),
        }
    }

    /// Number of the last block.
    pub const fn last_block(&self) -> BlockNumber {
        match self {
            Self::Single { block, .. } => *block,
            Self::Multiple(outcome) => outcome.last_block(),
        }
    }

    /// Returns a reference to the [`BundleState`].
    pub const fn state(&self) -> &BundleState {
        match self {
            Self::Single { outcome, .. } => &outcome.state,
            Self::Multiple(outcome) => &outcome.bundle,
        }
    }

    /// Returns an iterator over receipt sets for each block.
    pub fn receipts(&self) -> impl Iterator<Item = &Vec<R>> {
        match self {
            Self::Single { outcome, .. } => {
                Either::Left(core::iter::once(&outcome.result.receipts))
            }
            Self::Multiple(outcome) => Either::Right(outcome.receipts.iter()),
        }
    }
}

impl<'a, R> From<&'a ExecutionOutcome<R>> for WriteStateInput<'a, R> {
    fn from(outcome: &'a ExecutionOutcome<R>) -> Self {
        Self::Multiple(outcome)
    }
}

/// A trait specifically for writing state changes or reverts
pub trait StateWriter {
    /// Receipt type included into [`ExecutionOutcome`].
    type Receipt: 'static;

    /// Write the state and optionally receipts to the database.
    ///
    /// Use `config` to skip writing certain data types when they are written elsewhere.
    fn write_state<'a>(
        &self,
        execution_outcome: impl Into<WriteStateInput<'a, Self::Receipt>>,
        is_value_known: OriginalValuesKnown,
        config: StateWriteConfig,
    ) -> ProviderResult<()>;

    /// Write state reverts to the database.
    ///
    /// NOTE: Reverts will delete all wiped storage from plain state.
    ///
    /// Use `config` to skip writing certain data types when they are written elsewhere.
    fn write_state_reverts(
        &self,
        reverts: PlainStateReverts,
        first_block: BlockNumber,
        config: StateWriteConfig,
    ) -> ProviderResult<()>;

    /// Write state changes to the database.
    fn write_state_changes(&self, changes: StateChangeset) -> ProviderResult<()>;

    /// Writes the hashed state changes to the database
    fn write_hashed_state(&self, hashed_state: &HashedPostStateSorted) -> ProviderResult<()>;

    /// Remove the block range of state above the given block. The state of the passed block is not
    /// removed.
    fn remove_state_above(&self, block: BlockNumber) -> ProviderResult<()>;

    /// Take the block range of state, recreating the [`ExecutionOutcome`]. The state of the passed
    /// block is not removed.
    fn take_state_above(
        &self,
        block: BlockNumber,
    ) -> ProviderResult<ExecutionOutcome<Self::Receipt>>;
}

/// Configuration for what to write to the database (MDBX) when calling
/// [`StateWriter::write_state`].
///
/// Some types (receipts, changesets) may be written directly to
/// static files instead of the database depending on the storage settings. This config allows
/// skipping those types in the database write to avoid duplication.
#[derive(Debug, Clone, Copy)]
pub struct StateWriteConfig {
    /// Whether to write receipts to the database.
    ///
    /// Set to `false` when receipts are being written to static files instead.
    pub write_receipts: bool,
    /// Whether to write account changesets to the database.
    ///
    /// Set to `false` when account changesets are being written to static files instead.
    pub write_account_changesets: bool,
    /// Whether to write storage changesets to the database.
    ///
    /// Set to `false` when storage changesets are being written to static files instead.
    pub write_storage_changesets: bool,
}

impl Default for StateWriteConfig {
    fn default() -> Self {
        Self {
            write_receipts: true,
            write_account_changesets: true,
            write_storage_changesets: true,
        }
    }
}
