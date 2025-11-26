use std::vec;
use reth_storage_api::HeaderProvider;
use reth_primitives_traits::NodePrimitives;
use reth_rpc_convert::transaction::{ConvertReceiptInput, ReceiptConverter};
use reth_rpc_eth_types::receipt::build_receipt;
use alloy_rpc_types_eth::TransactionReceipt;
use alloy_serde::WithOtherFields;
use reth_rpc_eth_api::FromEthApiError;
use alloy_consensus::transaction::TxHashRef;
use alloy_consensus::Transaction;

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

            // Extract the transaction hash, 'to' address, and next_log_index before moving input
            // We need to compute these eagerly to avoid lifetime issues
            let tx_hash = {
                use alloy_eips::eip2718::Encodable2718;
                let mut buf = Vec::new();
                input.tx.encode_2718(&mut buf);
                alloy_primitives::keccak256(&buf)
            };
            // For `.to()`, we need to handle the case where it returns a reference
            // Since Address is Copy, we can just dereference and store
            // TODO: Fix lifetime issue when uncommenting the usage below
            // let tx_to = input.tx.deref().to().map(|addr| *addr);
            let next_log_index_val = input.next_log_index;

            // Build the receipt using the standard build_receipt function
            // The lambda converts the ArbReceipt to a ReceiptEnvelope, preserving cumulative gas and logs
            let mut base_receipt = build_receipt(input, None, |receipt, _next_log_index, _meta| {
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

                // Wrap in the correct envelope type based on transaction type
                // Note: alloy_consensus::ReceiptEnvelope only supports standard Ethereum types
                // Arbitrum-specific types (0x64-0x6a) will use Legacy for now as a workaround
                match tx_type_u8 {
                    0 => alloy_consensus::ReceiptEnvelope::Legacy(consensus_receipt),
                    1 => alloy_consensus::ReceiptEnvelope::Eip2930(consensus_receipt),
                    2 => alloy_consensus::ReceiptEnvelope::Eip1559(consensus_receipt),
                    3 => alloy_consensus::ReceiptEnvelope::Eip4844(consensus_receipt),
                    4 => alloy_consensus::ReceiptEnvelope::Eip7702(consensus_receipt),
                    // Arbitrum types: 0x64 (Deposit), 0x65 (Unsigned), 0x66 (Contract),
                    // 0x68 (Retry), 0x69 (SubmitRetryable), 0x6a (Internal)
                    // These need proper envelope variants - for now use Legacy as placeholder
                    _ => alloy_consensus::ReceiptEnvelope::Legacy(consensus_receipt),
                }
            });

            // Fix the 'from' field to use the correctly recovered signer
            // This is necessary because build_receipt's tx.signer() returns Address::ZERO
            base_receipt.from = correct_signer;

            // For internal transactions (0x6a) and deposit transactions (0x64), fix the receipt fields
            // These transaction types have special handling in Arbitrum
            if tx_type_u8 == 0x6a {
                // Internal transactions: gasUsed should be 0, to should be ArbOS address
                base_receipt.gas_used = 0;
                // TODO: Fix type mismatch - tx_to is Option<FixedBytes<20>> but need Option<Address>
                // base_receipt.to = tx_to;
                tracing::debug!(
                    target: "arb-reth::rpc-receipt",
                    tx_hash = ?tx_hash,
                    "Fixed internal tx receipt: gasUsed=0, to={:?}",
                    base_receipt.to
                );
            } else if tx_type_u8 == 0x64 {
                // Deposit transactions: gasUsed should be 0
                base_receipt.gas_used = 0;
                // TODO: Fix type mismatch - tx_to is Option<FixedBytes<20>> but need Option<Address>
                // base_receipt.to = tx_to;
                tracing::debug!(
                    target: "arb-reth::rpc-receipt",
                    tx_hash = ?tx_hash,
                    "Fixed deposit tx receipt: gasUsed=0"
                );
            }

            // The base_receipt now has all correct data including the from field
            // Convert from TransactionReceipt<ReceiptEnvelope> to TransactionReceipt (with default Log type)
            // We need to map the inner ReceiptEnvelope type
            use alloy_consensus::TxReceipt;
            use alloy_rpc_types_eth::Log as RpcLog;

            // Extract data from the consensus envelope
            let status = base_receipt.inner.status();
            let cumulative_gas_used = base_receipt.inner.cumulative_gas_used();
            let logs_bloom = base_receipt.inner.bloom();
            let consensus_logs = base_receipt.inner.logs();

            // Convert consensus logs (alloy_primitives::Log) to RPC logs (alloy_rpc_types_eth::Log)
            let rpc_logs: Vec<RpcLog> = consensus_logs.iter().enumerate().map(|(idx, log)| {
                RpcLog {
                    inner: alloy_primitives::Log {
                        address: log.address,
                        data: log.data.clone(),
                    },
                    block_hash: base_receipt.block_hash,
                    block_number: base_receipt.block_number,
                    block_timestamp: None,
                    transaction_hash: Some(base_receipt.transaction_hash),
                    transaction_index: base_receipt.transaction_index,
                    log_index: Some((next_log_index_val + idx) as u64),
                    removed: false,
                }
            }).collect();

            // Create ReceiptEnvelope<RpcLog>
            let receipt_with_bloom = alloy_consensus::ReceiptWithBloom {
                receipt: alloy_consensus::Receipt {
                    status: alloy_consensus::Eip658Value::Eip658(status),
                    cumulative_gas_used,
                    logs: rpc_logs.clone(),
                },
                logs_bloom,
            };
            // Create the correct envelope type based on transaction type
            // Same workaround as above - Arbitrum types use Legacy until proper support is added
            let inner_envelope = match tx_type_u8 {
                0 => alloy_consensus::ReceiptEnvelope::Legacy(receipt_with_bloom),
                1 => alloy_consensus::ReceiptEnvelope::Eip2930(receipt_with_bloom),
                2 => alloy_consensus::ReceiptEnvelope::Eip1559(receipt_with_bloom),
                3 => alloy_consensus::ReceiptEnvelope::Eip4844(receipt_with_bloom),
                4 => alloy_consensus::ReceiptEnvelope::Eip7702(receipt_with_bloom),
                _ => alloy_consensus::ReceiptEnvelope::Legacy(receipt_with_bloom),
            };

            // Construct the TransactionReceipt with the correct type
            let plain_receipt = TransactionReceipt {
                inner: inner_envelope,
                transaction_hash: base_receipt.transaction_hash,
                transaction_index: base_receipt.transaction_index,
                block_hash: base_receipt.block_hash,
                block_number: base_receipt.block_number,
                gas_used: base_receipt.gas_used,
                effective_gas_price: base_receipt.effective_gas_price,
                blob_gas_used: base_receipt.blob_gas_used,
                blob_gas_price: base_receipt.blob_gas_price,
                from: base_receipt.from,
                to: base_receipt.to,
                contract_address: base_receipt.contract_address,
            };
            out.push(plain_receipt);
        }
        Ok(out)
    }
}
