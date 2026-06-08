use alloc::{borrow::Cow, vec::Vec};
use alloy_consensus::transaction::Either;
use alloy_primitives::{Address, BlockNumber, B256, U256};
use reth_execution_types::{BlockExecutionOutput, Evm2BundleState, ExecutionOutcome};
use reth_primitives_traits::{Account, Bytecode};
use reth_storage_errors::provider::ProviderResult;
use reth_trie_common::HashedPostStateSorted;

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

    /// Returns a reference to the execution bundle state.
    pub fn state(&self) -> Cow<'_, Evm2BundleState> {
        match self {
            Self::Single { outcome, block } => {
                Cow::Owned(Evm2BundleState::from_state_source(*block, &outcome.state))
            }
            Self::Multiple(outcome) => Cow::Borrowed(&outcome.bundle),
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

/// Whether the original values in a bundle are known to match the database state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OriginalValuesKnown {
    /// Original values are known.
    Yes,
    /// Original values are not known for sure.
    No,
}

impl OriginalValuesKnown {
    /// Returns true if the original value is not known for sure.
    pub const fn is_not_known(&self) -> bool {
        matches!(self, Self::No)
    }
}

/// State changes for inclusion into the database.
#[derive(Clone, Debug, Default)]
pub struct StateChangeset {
    /// Account changes.
    pub accounts: Vec<(Address, Option<Account>)>,
    /// Storage changes.
    pub storage: Vec<PlainStorageChangeset>,
    /// Changed contracts by bytecode hash.
    pub contracts: Vec<(B256, Bytecode)>,
}

/// Plain storage changeset.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PlainStorageChangeset {
    /// Address of the account.
    pub address: Address,
    /// Whether storage should be wiped before applying changed slots.
    pub wipe_storage: bool,
    /// Storage key value pairs.
    pub storage: Vec<(U256, U256)>,
}

/// Storage revert value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RevertToSlot {
    /// Revert the slot to the previous value.
    Some(U256),
    /// Revert the slot to the value loaded from wiped storage.
    Destroyed,
}

impl RevertToSlot {
    /// Returns the previous value represented by this revert.
    pub const fn to_previous_value(self) -> U256 {
        match self {
            Self::Some(value) => value,
            Self::Destroyed => U256::ZERO,
        }
    }
}

impl Default for RevertToSlot {
    fn default() -> Self {
        Self::Some(U256::ZERO)
    }
}

/// Plain storage revert.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PlainStorageRevert {
    /// Address of the account.
    pub address: Address,
    /// Whether storage was wiped.
    pub wiped: bool,
    /// Storage keys and old values.
    pub storage_revert: Vec<(U256, RevertToSlot)>,
}

/// Plain state reverts grouped by block.
#[derive(Clone, Debug, Default)]
pub struct PlainStateReverts {
    /// Account reverts per block.
    pub accounts: Vec<Vec<(Address, Option<Account>)>>,
    /// Storage reverts per block.
    pub storage: Vec<Vec<PlainStorageRevert>>,
}

impl PlainStateReverts {
    /// Constructs state reverts with pre-allocated block capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self { accounts: Vec::with_capacity(capacity), storage: Vec::with_capacity(capacity) }
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
