use alloy_consensus::{Eip658Value, Header, Receipt};
use core::fmt;
use reth_scroll_primitives::{ScrollReceipt, ScrollTransactionSigned};
use revm_primitives::{ExecutionResult, U256};
use scroll_alloy_consensus::{ScrollTransactionReceipt, ScrollTxType};

/// Context for building a receipt.
#[derive(Debug)]
pub struct ReceiptBuilderCtx<'a, T> {
    /// Block header.
    pub header: &'a Header,
    /// Transaction
    pub tx: &'a T,
    /// Result of transaction execution.
    pub result: ExecutionResult,
    /// Cumulative gas used.
    pub cumulative_gas_used: u64,
    /// L1 fee.
    pub l1_fee: U256,
}

/// Type that knows how to build a receipt based on execution result.
pub trait ScrollReceiptBuilder<T>: fmt::Debug + Send + Sync + Unpin + 'static {
    /// Receipt type.
    type Receipt: Send + Sync + Clone + Unpin + 'static;

    /// Builds a receipt given a transaction and the result of the execution.
    fn build_receipt(&self, ctx: ReceiptBuilderCtx<'_, T>) -> Self::Receipt;
}

/// Basic builder for receipts of [`ScrollTransactionSigned`].
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct BasicScrollReceiptBuilder;

impl ScrollReceiptBuilder<ScrollTransactionSigned> for BasicScrollReceiptBuilder {
    type Receipt = ScrollReceipt;

    fn build_receipt(&self, ctx: ReceiptBuilderCtx<'_, ScrollTransactionSigned>) -> Self::Receipt {
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
            ScrollTxType::L1Message => ScrollReceipt::L1Message(inner),
        }
    }
}
