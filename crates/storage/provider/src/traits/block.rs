use alloy_primitives::BlockNumber;
use reth_execution_types::ExecutionOutcome;
use reth_storage_errors::provider::ProviderResult;

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
