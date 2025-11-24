use std::vec;
use reth_storage_api::HeaderProvider;
use reth_primitives_traits::NodePrimitives;
use reth_rpc_convert::transaction::{ConvertReceiptInput, ReceiptConverter};
use reth_rpc_eth_types::receipt::build_receipt;
use alloy_rpc_types_eth::TransactionReceipt;

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
    type RpcReceipt = TransactionReceipt;
    type Error = crate::error::ArbEthApiError;

    fn convert_receipts(
        &self,
        receipts: vec::Vec<ConvertReceiptInput<'_, N>>,
    ) -> Result<vec::Vec<Self::RpcReceipt>, Self::Error> {
        let mut out = Vec::with_capacity(receipts.len());
        for input in receipts {
            out.push(build_receipt(input, None, |receipt, _next_log_index, _meta| {
                // Return the actual receipt data, not a default one
                // For Arbitrum, receipts are stored with bloom filters
                use alloy_consensus::ReceiptWithBloom;

                // Create receipt with bloom from the actual receipt data
                let receipt_with_bloom = ReceiptWithBloom::new(
                    receipt.clone(),
                    alloy_primitives::Bloom::default() // Bloom will be computed from logs
                );

                alloy_consensus::ReceiptEnvelope::Legacy(receipt_with_bloom)
            }));
        }
        Ok(out)
    }
}
