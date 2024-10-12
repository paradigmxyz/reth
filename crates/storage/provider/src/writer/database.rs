use alloy_primitives::{BlockNumber, TxNumber};
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW},
    tables,
};
use reth_errors::ProviderResult;
use reth_primitives::Receipt;
use reth_storage_api::ReceiptWriter;

pub(crate) struct DatabaseWriter<'a, W>(pub(crate) &'a mut W);

impl<W> ReceiptWriter for DatabaseWriter<'_, W>
where
    W: DbCursorRO<tables::Receipts> + DbCursorRW<tables::Receipts>,
{
    fn append_block_receipts(
        &mut self,
        first_tx_index: TxNumber,
        _: BlockNumber,
        receipts: Vec<Option<Receipt>>,
    ) -> ProviderResult<()> {
        for (tx_idx, receipt) in receipts.into_iter().enumerate() {
            if let Some(receipt) = receipt {
                self.0.append(first_tx_index + tx_idx as u64, receipt)?;
            }
        }
        Ok(())
    }
}
