use reth_execution_types::ExecutionOutcome;
use reth_storage_errors::provider::ProviderResult;
use revm::db::OriginalValuesKnown;

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
