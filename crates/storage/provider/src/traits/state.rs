use alloy_primitives::BlockNumber;
use reth_execution_types::ExecutionOutcome;
use reth_storage_errors::provider::ProviderResult;
use reth_trie::HashedPostStateSorted;
use revm::db::{
    states::{PlainStateReverts, StateChangeset},
    OriginalValuesKnown,
};

use super::StorageLocation;

/// A helper trait for [`ExecutionOutcome`] to write state and receipts to storage.
pub trait StateWriter {
    /// Write the data and receipts to the database or static files if `static_file_producer` is
    /// `Some`. It should be `None` if there is any kind of pruning/filtering over the receipts.
    fn write_to_storage(
        &self,
        execution_outcome: ExecutionOutcome,
        is_value_known: OriginalValuesKnown,
        write_receipts_to: StorageLocation,
    ) -> ProviderResult<()>;
}

/// A trait specifically for writing state changes or reverts
pub trait StateChangeWriter {
    /// Write state reverts to the database.
    ///
    /// NOTE: Reverts will delete all wiped storage from plain state.
    fn write_state_reverts(
        &self,
        reverts: PlainStateReverts,
        first_block: BlockNumber,
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
    fn take_state_above(&self, block: BlockNumber) -> ProviderResult<ExecutionOutcome>;
}
