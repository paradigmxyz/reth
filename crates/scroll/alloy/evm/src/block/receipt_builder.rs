use alloy_evm::Evm;
use alloy_primitives::U256;
use core::fmt::Debug;
use revm::context::result::ExecutionResult;

/// Context for building a receipt.
#[derive(Debug)]
pub struct ReceiptBuilderCtx<'a, T, E: Evm> {
    /// Transaction
    pub tx: &'a T,
    /// Result of transaction execution.
    pub result: ExecutionResult<E::HaltReason>,
    /// Cumulative gas used.
    pub cumulative_gas_used: u64,
    /// L1 fee.
    pub l1_fee: U256,
}

/// Type that knows how to build a receipt based on execution result.
#[auto_impl::auto_impl(&, Arc)]
pub trait ScrollReceiptBuilder: Debug {
    /// Transaction type.
    type Transaction;
    /// Receipt type.
    type Receipt;

    /// Builds a receipt given a transaction and the result of the execution.
    fn build_receipt<'a, E: Evm>(
        &self,
        ctx: ReceiptBuilderCtx<'a, Self::Transaction, E>,
    ) -> Self::Receipt;
}
