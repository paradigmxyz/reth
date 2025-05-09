use alloy_consensus::{Eip658Value, Receipt};
use alloy_evm::Evm;
use reth_scroll_primitives::{ScrollReceipt, ScrollTransactionSigned};
use scroll_alloy_consensus::{ScrollTransactionReceipt, ScrollTxType};
use scroll_alloy_evm::{ReceiptBuilderCtx, ScrollReceiptBuilder};

/// Basic builder for receipts of [`ScrollTransactionSigned`].
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct ScrollRethReceiptBuilder;

impl ScrollReceiptBuilder for ScrollRethReceiptBuilder {
    type Transaction = ScrollTransactionSigned;
    type Receipt = ScrollReceipt;

    fn build_receipt<E: Evm>(
        &self,
        ctx: ReceiptBuilderCtx<'_, ScrollTransactionSigned, E>,
    ) -> Self::Receipt {
        let inner = Receipt {
            // Success flag was added in `EIP-658: Embedding transaction status code in
            // receipts`.
            status: Eip658Value::Eip658(ctx.result.is_success()),
            cumulative_gas_used: ctx.cumulative_gas_used,
            logs: ctx.result.into_logs(),
        };
        let into_scroll_receipt = |inner: Receipt| ScrollTransactionReceipt::new(inner, ctx.l1_fee);

        match ctx.tx.tx_type() {
            ScrollTxType::Legacy => ScrollReceipt::Legacy(into_scroll_receipt(inner)),
            ScrollTxType::Eip2930 => ScrollReceipt::Eip2930(into_scroll_receipt(inner)),
            ScrollTxType::Eip1559 => ScrollReceipt::Eip1559(into_scroll_receipt(inner)),
            ScrollTxType::Eip7702 => ScrollReceipt::Eip7702(into_scroll_receipt(inner)),
            ScrollTxType::L1Message => ScrollReceipt::L1Message(inner),
        }
    }
}
