use crate::providers::StaticFileProviderRWRefMut;
use reth_errors::ProviderResult;
use reth_primitives::{BlockNumber, Receipt, StaticFileSegment, TxNumber};
use reth_storage_api::ReceiptWriter;

pub(crate) struct StaticFileWriter<'a, W>(pub(crate) &'a mut W);

impl<'a> ReceiptWriter for StaticFileWriter<'a, StaticFileProviderRWRefMut<'_>> {
    fn append_block_receipts(
        &mut self,
        first_tx_index: TxNumber,
        block_number: BlockNumber,
        receipts: Vec<Option<Receipt>>,
    ) -> ProviderResult<()> {
        // Increment block on static file header.
        self.0.increment_block(StaticFileSegment::Receipts, block_number)?;
        let receipts = receipts.into_iter().enumerate().map(|(tx_idx, receipt)| {
            Ok((
                first_tx_index + tx_idx as u64,
                receipt.expect("receipt should not be filtered when saving to static files."),
            ))
        });
        self.0.append_receipts(receipts)?;
        Ok(())
    }
}
