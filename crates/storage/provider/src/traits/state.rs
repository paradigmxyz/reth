use alloy_primitives::BlockNumber;
use reth_execution_types::ExecutionOutcome;
use reth_storage_errors::provider::ProviderResult;
use reth_trie::HashedPostStateSorted;
use revm::db::{
    states::{PlainStateReverts, StateChangeset},
    OriginalValuesKnown,
};
use std::ops::RangeInclusive;

/// A helper trait for [`ExecutionOutcome`] to write state and receipts to storage.
pub trait StateWriter {
    /// Write the data and receipts to the database or static files if `static_file_producer` is
    /// `Some`. It should be `None` if there is any kind of pruning/filtering over the receipts.
    fn write_to_storage(
        &mut self,
        execution_outcome: ExecutionOutcome,
        is_value_known: OriginalValuesKnown,
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

    /// Remove the block range of state.
    fn remove_state(&self, range: RangeInclusive<BlockNumber>) -> ProviderResult<()>;

    /// Take the block range of state, recreating the [`ExecutionOutcome`].
    fn take_state(&self, range: RangeInclusive<BlockNumber>) -> ProviderResult<ExecutionOutcome>;
}
