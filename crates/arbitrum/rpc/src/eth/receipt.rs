use std::vec;
use reth_storage_api::HeaderProvider;
use reth_primitives_traits::NodePrimitives;
use reth_rpc_convert::transaction::{ConvertReceiptInput, ReceiptConverter};
use reth_rpc_eth_types::receipt::build_receipt;
use alloy_rpc_types_eth::TransactionReceipt;
use alloy_serde::WithOtherFields;
use reth_rpc_eth_api::FromEthApiError;

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

            // Recover the signer directly from the inner transaction
            // This works around an issue where tx.signer() returns Address::ZERO
            use alloy_consensus::transaction::SignerRecoverable;
            use core::ops::Deref;
            let correct_signer = input.tx.deref().recover_signer()
                .map_err(|_| crate::error::ArbEthApiError::from_eth_err(reth_rpc_eth_types::EthApiError::InvalidTransactionSignature))?;

            // Build the receipt using the standard build_receipt function
            // The lambda converts the ArbReceipt to a ReceiptEnvelope, preserving cumulative gas and logs
            let mut base_receipt = build_receipt(input, None, |receipt, next_log_index, meta| {
                use alloy_consensus::TxReceipt;

                // Extract receipt data from the ArbReceipt
                let status = receipt.status();
                let cumulative_gas_used = receipt.cumulative_gas_used();
                let logs = receipt.logs();
                let bloom = receipt.bloom();

                // Use the logs directly (they're already alloy_primitives::Log)
                // Don't convert to RPC format here - that happens in build_receipt
                let primitive_logs = logs.to_vec();

                // Create a consensus receipt with the actual data
                let consensus_receipt = alloy_consensus::ReceiptWithBloom {
                    receipt: alloy_consensus::Receipt {
                        status: alloy_consensus::Eip658Value::Eip658(status),
                        cumulative_gas_used,
                        logs: primitive_logs,
                    },
                    logs_bloom: bloom,
                };

                // Wrap in a Legacy envelope (Arbitrum uses legacy receipt format for most txs)
                alloy_consensus::ReceiptEnvelope::Legacy(consensus_receipt)
            });

            // Fix the 'from' field to use the correctly recovered signer
            // This is necessary because build_receipt's tx.signer() returns Address::ZERO
            base_receipt.from = correct_signer;

            // The base_receipt now has all correct data including the from field
            // Convert to the expected type using Into
            out.push(base_receipt.into());
        }
        Ok(out)
    }
}
