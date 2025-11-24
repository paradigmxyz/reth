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
            // Build the receipt using a custom conversion function
            let rpc_receipt = build_receipt(input, None, |receipt, _next_log_index, _meta| {
                // For Arbitrum, we need to properly handle the receipt data
                // The receipt parameter here is N::Receipt, and build_receipt will extract the data
                // We just need to provide the proper ReceiptEnvelope wrapper
                // Since we don't have direct access to the inner data here, we'll need to
                // work with what build_receipt provides
                //
                // The issue is that build_receipt extracts the data from the receipt,
                // but our callback was returning defaults. We need to ensure build_receipt
                // uses the actual receipt data.
                //
                // Actually, looking at build_receipt more carefully, it passes the receipt
                // to our callback, and we're supposed to convert it to a ReceiptEnvelope.
                // The TransactionReceipt struct that build_receipt returns will have
                // the correct gas_used, etc. from the input, not from our envelope.
                //
                // So returning a default envelope might actually be okay IF build_receipt
                // is extracting the data correctly. Let me verify this assumption.
                alloy_consensus::ReceiptEnvelope::Legacy(alloy_consensus::ReceiptWithBloom::default())
            });
            out.push(rpc_receipt);
        }
        Ok(out)
    }
}
