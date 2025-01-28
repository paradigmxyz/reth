use alloy_consensus::{Eip658Value, Header, Receipt};
use core::fmt;
use op_alloy_consensus::{OpDepositReceipt, OpTxType};
use reth_optimism_primitives::{OpReceipt, OpTransactionSigned};
use revm_primitives::ExecutionResult;

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
}

/// Type that knows how to build a receipt based on execution result.
pub trait OpReceiptBuilder<T>: fmt::Debug + Send + Sync + Unpin + 'static {
    /// Receipt type.
    type Receipt: Send + Sync + Clone + Unpin + 'static;

    /// Builds a receipt given a transaction and the result of the execution.
    ///
    /// Note: this method should return `Err` if the transaction is a deposit transaction. In that
    /// case, the `build_deposit_receipt` method will be called.
    fn build_receipt<'a>(
        &self,
        ctx: ReceiptBuilderCtx<'a, T>,
    ) -> Result<Self::Receipt, ReceiptBuilderCtx<'a, T>>;

    /// Builds receipt for a deposit transaction.
    fn build_deposit_receipt(&self, inner: OpDepositReceipt) -> Self::Receipt;
}

/// Basic builder for receipts of [`OpTransactionSigned`].
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct BasicOpReceiptBuilder;

impl OpReceiptBuilder<OpTransactionSigned> for BasicOpReceiptBuilder {
    type Receipt = OpReceipt;

    fn build_receipt<'a>(
        &self,
        ctx: ReceiptBuilderCtx<'a, OpTransactionSigned>,
    ) -> Result<Self::Receipt, ReceiptBuilderCtx<'a, OpTransactionSigned>> {
        match ctx.tx.tx_type() {
            OpTxType::Deposit => Err(ctx),
            ty => {
                let receipt = Receipt {
                    // Success flag was added in `EIP-658: Embedding transaction status code in
                    // receipts`.
                    status: Eip658Value::Eip658(ctx.result.is_success()),
                    cumulative_gas_used: ctx.cumulative_gas_used,
                    logs: ctx.result.into_logs(),
                };

                Ok(match ty {
                    OpTxType::Legacy => OpReceipt::Legacy(receipt),
                    OpTxType::Eip1559 => OpReceipt::Eip1559(receipt),
                    OpTxType::Eip2930 => OpReceipt::Eip2930(receipt),
                    OpTxType::Eip7702 => OpReceipt::Eip7702(receipt),
                    OpTxType::Deposit => unreachable!(),
                })
            }
        }
    }

    fn build_deposit_receipt(&self, inner: OpDepositReceipt) -> Self::Receipt {
        OpReceipt::Deposit(inner)
    }
}
