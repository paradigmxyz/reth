use alloy_primitives::BlockNumber;
use reth_execution_types::ExecutionOutcome;
use reth_storage_errors::provider::ProviderResult;
use reth_trie_common::HashedPostStateSorted;
use revm_database::{
    states::{PlainStateReverts, StateChangeset},
    OriginalValuesKnown,
};

/// A trait specifically for writing state changes or reverts
pub trait StateWriter {
    /// Receipt type included into [`ExecutionOutcome`].
    type Receipt;

    /// Write the state and optionally receipts to the database.
    ///
    /// Use `config` to skip writing certain data types when they are written elsewhere.
    fn write_state(
        &self,
        execution_outcome: &ExecutionOutcome<Self::Receipt>,
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

/// Configuration for what to write when calling [`StateWriter::write_state`].
///
/// Used to skip writing certain data types, when they are being written separately.
#[derive(Debug, Clone, Copy)]
pub struct StateWriteConfig {
    /// Whether to write receipts.
    pub write_receipts: bool,
    /// Whether to write account changesets.
    pub write_account_changesets: bool,
}

impl Default for StateWriteConfig {
    fn default() -> Self {
        Self { write_receipts: true, write_account_changesets: true }
    }
}
