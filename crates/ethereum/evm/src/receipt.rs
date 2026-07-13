use alloy_consensus::TxType;
use evm2::TxResult;
use reth_ethereum_primitives::Receipt;
use reth_evm::{ReceiptBuilder, ReceiptBuilderCtx};

/// A builder that produces Reth [`Receipt`] values from evm2 transaction results.
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct RethReceiptBuilder;

impl ReceiptBuilder<TxType, TxResult> for RethReceiptBuilder {
    type Receipt = Receipt;

    fn build_receipt(&self, ctx: ReceiptBuilderCtx<TxType, TxResult>) -> Receipt {
        let ReceiptBuilderCtx { tx_type, result, cumulative_gas_used } = ctx;
        Receipt { tx_type, success: result.status, cumulative_gas_used, logs: result.logs }
    }
}
