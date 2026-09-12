use alloy_consensus::TxType;
use alloy_evm::{
    block::BlockExecutionError,
    eth::receipt_builder::{AlloyReceiptBuilder, ReceiptBuilder, ReceiptBuilderCtx},
};
use reth_ethereum_primitives::{Receipt, TransactionSigned};
use reth_evm::Evm;

/// A builder that operates on Reth primitive types, specifically [`TransactionSigned`] and
/// [`Receipt`].
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct RethReceiptBuilder;

impl ReceiptBuilder for RethReceiptBuilder {
    type Transaction = TransactionSigned;
    type Receipt = Receipt;

    fn build_receipt<E: Evm>(
        &self,
        ctx: ReceiptBuilderCtx<'_, TxType, E>,
    ) -> Result<Self::Receipt, BlockExecutionError> {
        AlloyReceiptBuilder::default().build_receipt(ctx).map(Into::into)
    }
}
