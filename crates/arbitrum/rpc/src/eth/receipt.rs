use std::vec;
use reth_storage_api::HeaderProvider;
use reth_primitives_traits::NodePrimitives;
use reth_rpc_convert::transaction::{ConvertReceiptInput, ReceiptConverter};
use reth_rpc_eth_types::receipt::build_receipt;
use alloy_rpc_types_eth::TransactionReceipt;
use alloy_serde::WithOtherFields;

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
    type RpcReceipt = WithOtherFields<TransactionReceipt>;
    type Error = crate::error::ArbEthApiError;

    fn convert_receipts(
        &self,
        receipts: vec::Vec<ConvertReceiptInput<'_, N>>,
    ) -> Result<vec::Vec<Self::RpcReceipt>, Self::Error> {
        let mut out = Vec::with_capacity(receipts.len());
        for input in receipts {
            // Get the transaction type u8 value using the Typed2718 trait
            // This preserves Arbitrum-specific types like 0x6a (internal), 0x64 (deposit), etc.
            let tx_type_u8 = {
                use alloy_consensus::Typed2718;
                input.tx.ty()
            };

            // Build the receipt using the standard build_receipt function
            let base_receipt = build_receipt(input, None, |receipt, _next_log_index, _meta| {
                // Return a default envelope - the actual receipt data is extracted by build_receipt
                // The envelope type doesn't matter since we'll override it with WithOtherFields
                alloy_consensus::ReceiptEnvelope::Legacy(alloy_consensus::ReceiptWithBloom::default())
            });

            // Wrap with WithOtherFields to ensure the correct transaction type is in the JSON response
            // The standard receipt envelope doesn't support Arbitrum-specific types like 0x6a
            let mut wrapped = WithOtherFields::new(base_receipt);
            // Add the type field explicitly to the "other" fields
            // This ensures it serializes correctly for Arbitrum transaction types
            wrapped.other.insert("type".to_string(), serde_json::Value::String(format!("0x{:x}", tx_type_u8)));

            out.push(wrapped);
        }
        Ok(out)
    }
}
