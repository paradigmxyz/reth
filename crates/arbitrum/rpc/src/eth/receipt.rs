use std::vec;
use reth_storage_api::HeaderProvider;
use reth_primitives_traits::NodePrimitives;
use reth_rpc_convert::transaction::{ConvertReceiptInput, ReceiptConverter};
use alloy_consensus::ReceiptEnvelope;
use reth_rpc_eth_types::receipt::build_receipt;

#[derive(Clone, Debug)]
pub struct ArbReceiptConverter<P> {
    _provider: P,
}

impl<P> ArbReceiptConverter<P> {
    pub fn new(provider: P) -> Self {
        Self { _provider: provider }
    }
}

impl<P, N> ReceiptConverter<N> for ArbReceiptConverter<P>
where
    P: HeaderProvider + Clone + Send + Sync + 'static + core::fmt::Debug,
    N: NodePrimitives,
{
    type RpcReceipt = alloy_rpc_types_eth::TransactionReceipt;
    type Error = crate::error::ArbEthApiError;

    fn convert_receipts(
        &self,
        receipts: vec::Vec<ConvertReceiptInput<'_, N>>,
    ) -> Result<vec::Vec<Self::RpcReceipt>, Self::Error> {
        use reth_arbitrum_primitives::{ArbTransactionSigned, ArbTypedTransaction};
        let mut out = Vec::with_capacity(receipts.len());
        for input in receipts {
            let ty_opt = input.tx.as_ref().downcast_ref::<ArbTransactionSigned>().map(|arb_tx| {
                match &**arb_tx {
                    ArbTypedTransaction::SubmitRetryable(_) => Some(arb_alloy_consensus::tx::ArbTxType::ArbitrumSubmitRetryableTx.as_u8()),
                    ArbTypedTransaction::Retry(_) => Some(arb_alloy_consensus::tx::ArbTxType::ArbitrumRetryTx.as_u8()),
                    ArbTypedTransaction::Internal(_) => Some(arb_alloy_consensus::tx::ArbTxType::ArbitrumInternalTx.as_u8()),
                    ArbTypedTransaction::Deposit(_) => Some(arb_alloy_consensus::tx::ArbTxType::ArbitrumDepositTx.as_u8()),
                    _ => None,
                }
            }).flatten();

            out.push(build_receipt(&input, None, |receipt_with_bloom| {
                if let Some(ty) = ty_opt {
                    ReceiptEnvelope::Eip2718 { ty, inner: receipt_with_bloom }
                } else {
                    ReceiptEnvelope::Legacy(receipt_with_bloom)
                }
            }));
        }
        Ok(out)
    }
}
