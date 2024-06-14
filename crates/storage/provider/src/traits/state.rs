use crate::providers::StaticFileProviderRWRefMut;
use reth_db_api::transaction::{DbTx, DbTxMut};
use reth_storage_errors::provider::ProviderResult;
use revm::db::OriginalValuesKnown;

/// A helper trait for [`ExecutionOutcome`](reth_execution_types::ExecutionOutcome) to
/// write state and receipts to storage.
pub trait StateWriter {
    /// Write the data and receipts to the database or static files if `static_file_producer` is
    /// `Some`. It should be `None` if there is any kind of pruning/filtering over the receipts.
    fn write_to_storage<TX>(
        self,
        tx: &TX,
        static_file_producer: Option<StaticFileProviderRWRefMut<'_>>,
        is_value_known: OriginalValuesKnown,
    ) -> ProviderResult<()>
    where
        TX: DbTxMut + DbTx;
}
