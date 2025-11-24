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
    type RpcReceipt = TransactionReceipt<alloy_consensus::ReceiptEnvelope>;
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
            // The lambda converts the ArbReceipt to a ReceiptEnvelope, preserving cumulative gas and logs
            let base_receipt = build_receipt(input, None, |receipt, next_log_index, meta| {
                use alloy_consensus::TxReceipt;

                // Extract receipt data from the ArbReceipt
                let status = receipt.status();
                let cumulative_gas_used = receipt.cumulative_gas_used();
                let logs = receipt.logs();
                let bloom = receipt.bloom();

                // Convert logs to RPC format
                let rpc_logs = alloy_rpc_types_eth::Log::collect_for_receipt(
                    next_log_index,
                    meta,
                    logs.to_vec()
                );

                // Create a consensus receipt with the actual data
                let consensus_receipt = alloy_consensus::ReceiptWithBloom {
                    receipt: alloy_consensus::Receipt {
                        status: alloy_consensus::Eip658Value::Eip658(status),
                        cumulative_gas_used,
                        logs: rpc_logs,
                    },
                    logs_bloom: bloom,
                };

                // Wrap in a Legacy envelope (Arbitrum uses legacy receipt format for most txs)
                alloy_consensus::ReceiptEnvelope::Legacy(consensus_receipt)
            });

            // The base_receipt already has all the data we need
            // The tx type is encoded in the envelope so no need for WithOtherFields
            // Convert to the expected type using Into
            out.push(base_receipt.into());
        }
        Ok(out)
    }
}
