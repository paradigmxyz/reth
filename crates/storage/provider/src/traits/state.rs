use alloy_primitives::BlockNumber;
use reth_execution_types::ExecutionOutcome;
use reth_storage_errors::provider::ProviderResult;
use reth_trie::{HashedPostState, HashedPostStateSorted};
use revm::db::{
    states::{PlainStateReverts, StateChangeset},
    OriginalValuesKnown,
};

use super::StorageLocation;

/// A trait specifically for writing state changes or reverts
pub trait StateWriter {
    /// Receipt type included into [`ExecutionOutcome`].
    type Receipt;

    /// Write the state and receipts to the database or static files if `static_file_producer` is
    /// `Some`. It should be `None` if there is any kind of pruning/filtering over the receipts.
    fn write_state(
        &self,
        execution_outcome: ExecutionOutcome<Self::Receipt>,
        is_value_known: OriginalValuesKnown,
        write_receipts_to: StorageLocation,
    ) -> ProviderResult<()>;

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
    fn remove_state_above(
        &self,
        block: BlockNumber,
        remove_receipts_from: StorageLocation,
    ) -> ProviderResult<()>;

    /// Take the block range of state, recreating the [`ExecutionOutcome`]. The state of the passed
    /// block is not removed.
    fn take_state_above(
        &self,
        block: BlockNumber,
        remove_receipts_from: StorageLocation,
    ) -> ProviderResult<ExecutionOutcome<Self::Receipt>>;
}

/// This just receives state, or [`ExecutionOutcome`], from the provider
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait StateReader: Send + Sync {
    /// Receipt type in [`ExecutionOutcome`].
    type Receipt: Send + Sync;

    /// Get the [`ExecutionOutcome`] for the given block
    fn get_state(
        &self,
        block: BlockNumber,
    ) -> ProviderResult<Option<ExecutionOutcome<Self::Receipt>>>;
}

/// This is responsible for fetching hashed state changes from a historical block, to the tip of the
/// chain.
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait HashedStateReader: Send + Sync {
    /// Get the [`HashedPostState`] changes from the given block, to the tip of the chain.
    fn get_hashed_reverts(&self, from: BlockNumber) -> ProviderResult<HashedPostState>;
}
