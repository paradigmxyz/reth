use alloy_consensus::TxType;
use evm2::{EvmTypes, TxResult};
use reth_ethereum_primitives::{Receipt, TransactionSigned};
use reth_evm::{ReceiptBuilder, ReceiptBuilderCtx};

/// A builder that produces Reth [`Receipt`] values from evm2 transaction results.
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct RethReceiptBuilder;

impl ReceiptBuilder for RethReceiptBuilder {
    type Transaction = TransactionSigned;
    type Receipt = Receipt;

    fn build_receipt<T: EvmTypes>(&self, ctx: ReceiptBuilderCtx<TxType, TxResult<T>>) -> Receipt {
        let ReceiptBuilderCtx { tx_type, result, cumulative_gas_used } = ctx;
        Receipt { tx_type, success: result.status, cumulative_gas_used, logs: result.logs }
    }
}
