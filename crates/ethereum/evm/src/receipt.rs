use alloy_consensus::{ReceiptEnvelope, TxType};
use evm2::{EvmTypes, TxResult};
use reth_ethereum_primitives::{Receipt, TransactionSigned};
use reth_evm::{BlockExecutionError, ReceiptBuilder, ReceiptBuilderCtx};

/// A builder that produces Reth [`Receipt`] values from evm2 transaction results.
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct RethReceiptBuilder;

impl ReceiptBuilder for RethReceiptBuilder {
    type Transaction = TransactionSigned;
    type Receipt = Receipt;

    fn build_receipt<T: EvmTypes>(
        &self,
        ctx: ReceiptBuilderCtx<TxType, TxResult<T>>,
    ) -> Result<Receipt, BlockExecutionError> {
        let ReceiptBuilderCtx { tx_type, result, cumulative_gas_used } = ctx;
        let receipt = if tx_type == TxType::Eip8141 {
            let frame =
                result.frame.ok_or_else(|| BlockExecutionError::other(MissingFrameReceipt))?;
            Receipt::from_envelope(ReceiptEnvelope::Eip8141(
                frame.into_receipt(cumulative_gas_used).into(),
            ))
        } else {
            Receipt::standard(tx_type, result.status, cumulative_gas_used, result.logs)
        };
        Ok(receipt)
    }
}

#[derive(Debug)]
struct MissingFrameReceipt;

impl core::fmt::Display for MissingFrameReceipt {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("frame transaction result is missing frame receipt data")
    }
}

impl core::error::Error for MissingFrameReceipt {}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::sync::Arc;
    use alloy_eips::eip8141::{FrameReceipt, FrameStatus};
    use alloy_primitives::Address;
    use evm2::{BaseEvmTypes, FrameTransactionResult};
    use reth_evm::InternalBlockExecutionError;

    #[test]
    fn frame_receipt_preserves_payer_statuses_and_cumulative_gas() {
        let payer = Address::with_last_byte(1);
        let receipts = vec![
            FrameReceipt { status: FrameStatus::Success, ..Default::default() },
            FrameReceipt { status: FrameStatus::Failure, ..Default::default() },
            FrameReceipt { status: FrameStatus::SkippedAtomicBatch, ..Default::default() },
        ];
        let result = TxResult::<BaseEvmTypes> {
            frame: Some(FrameTransactionResult {
                payer,
                receipts: receipts.clone(),
                ..Default::default()
            }),
            ..Default::default()
        };
        let receipt = RethReceiptBuilder
            .build_receipt::<BaseEvmTypes>(ReceiptBuilderCtx {
                tx_type: TxType::Eip8141,
                result,
                cumulative_gas_used: 42_000,
            })
            .unwrap();
        let payload = receipt.as_eip8141().unwrap();
        assert_eq!(payload.payer, payer);
        assert_eq!(payload.cumulative_gas_used, 42_000);
        assert_eq!(payload.frame_receipts, receipts);
    }

    #[test]
    fn frame_receipt_requires_native_frame_data() {
        let error = RethReceiptBuilder
            .build_receipt::<BaseEvmTypes>(ReceiptBuilderCtx {
                tx_type: TxType::Eip8141,
                result: TxResult::<BaseEvmTypes>::default(),
                cumulative_gas_used: 21_000,
            })
            .unwrap_err();
        assert!(matches!(
            error,
            BlockExecutionError::Internal(InternalBlockExecutionError::Other(source))
                if source.downcast_ref::<MissingFrameReceipt>().is_some()
        ));
    }

    #[test]
    fn reth_receipt_builder_implements_native_trait_through_arc() {
        let builder = Arc::new(RethReceiptBuilder);
        let receipt = evm2::block::ReceiptBuilder::build_receipt::<BaseEvmTypes>(
            &builder,
            ReceiptBuilderCtx {
                tx_type: TxType::Eip1559,
                result: TxResult::<BaseEvmTypes> { status: true, ..Default::default() },
                cumulative_gas_used: 42_000,
            },
        )
        .unwrap();
        assert_eq!(receipt, Receipt::standard(TxType::Eip1559, true, 42_000, vec![]));
    }
}
