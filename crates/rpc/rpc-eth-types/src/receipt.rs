//! RPC receipt response builder, extends a layer one receipt with layer two data.

use alloy_consensus::{ReceiptEnvelope, Transaction, TxReceipt};
use alloy_eips::eip7840::BlobParams;
use alloy_primitives::{Address, TxKind};
use alloy_rpc_types_eth::{Log, ReceiptWithBloom, TransactionReceipt};
use reth_chainspec::EthChainSpec;
use reth_ethereum_primitives::Receipt;
use reth_primitives_traits::NodePrimitives;
use reth_rpc_convert::transaction::{ConvertReceiptInput, ReceiptConverter};
use std::{borrow::Cow, sync::Arc};

use crate::EthApiError;

/// Builds an [`TransactionReceipt`] obtaining the inner receipt envelope from the given closure.
pub fn build_receipt<N, E>(
    input: &ConvertReceiptInput<'_, N>,
    blob_params: Option<BlobParams>,
    build_envelope: impl FnOnce(ReceiptWithBloom<alloy_consensus::Receipt<Log>>) -> E,
) -> TransactionReceipt<E>
where
    N: NodePrimitives,
{
    let ConvertReceiptInput { tx, meta, receipt, gas_used, next_log_index } = input;
    let from = tx.signer();

    let blob_gas_used = tx.blob_gas_used();
    // Blob gas price should only be present if the transaction is a blob transaction
    let blob_gas_price =
        blob_gas_used.and_then(|_| Some(blob_params?.calc_blob_fee(meta.excess_blob_gas?)));

    let status = receipt.status_or_post_state();
    let cumulative_gas_used = receipt.cumulative_gas_used();
    let logs_bloom = receipt.bloom();

    macro_rules! build_rpc_logs {
        ($logs:expr) => {
            $logs
                .enumerate()
                .map(|(tx_log_idx, log)| Log {
                    inner: log,
                    block_hash: Some(meta.block_hash),
                    block_number: Some(meta.block_number),
                    block_timestamp: Some(meta.timestamp),
                    transaction_hash: Some(meta.tx_hash),
                    transaction_index: Some(meta.index),
                    log_index: Some((next_log_index + tx_log_idx) as u64),
                    removed: false,
                })
                .collect()
        };
    }

    let logs = match receipt {
        Cow::Borrowed(r) => build_rpc_logs!(r.logs().iter().cloned()),
        Cow::Owned(r) => build_rpc_logs!(r.into_logs().into_iter()),
    };

    let rpc_receipt = alloy_rpc_types_eth::Receipt { status, cumulative_gas_used, logs };

    let (contract_address, to) = match tx.kind() {
        TxKind::Create => (Some(from.create(tx.nonce())), None),
        TxKind::Call(addr) => (None, Some(Address(*addr))),
    };

    TransactionReceipt {
        inner: build_envelope(ReceiptWithBloom { receipt: rpc_receipt, logs_bloom }),
        transaction_hash: meta.tx_hash,
        transaction_index: Some(meta.index),
        block_hash: Some(meta.block_hash),
        block_number: Some(meta.block_number),
        from,
        to,
        gas_used: *gas_used,
        contract_address,
        effective_gas_price: tx.effective_gas_price(meta.base_fee),
        // EIP-4844 fields
        blob_gas_price,
        blob_gas_used,
    }
}

/// Converter for Ethereum receipts.
#[derive(Debug)]
pub struct EthReceiptConverter<ChainSpec> {
    chain_spec: Arc<ChainSpec>,
}

impl<ChainSpec> Clone for EthReceiptConverter<ChainSpec> {
    fn clone(&self) -> Self {
        Self { chain_spec: self.chain_spec.clone() }
    }
}

impl<ChainSpec> EthReceiptConverter<ChainSpec> {
    /// Creates a new converter with the given chain spec.
    pub const fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self { chain_spec }
    }
}

impl<N, ChainSpec> ReceiptConverter<N> for EthReceiptConverter<ChainSpec>
where
    N: NodePrimitives<Receipt = Receipt>,
    ChainSpec: EthChainSpec + 'static,
{
    type Error = EthApiError;
    type RpcReceipt = TransactionReceipt;

    fn convert_receipts(
        &self,
        inputs: Vec<ConvertReceiptInput<'_, N>>,
    ) -> Result<Vec<Self::RpcReceipt>, Self::Error> {
        let mut receipts = Vec::with_capacity(inputs.len());

        for input in inputs {
            let tx_type = input.receipt.tx_type;
            let blob_params = self.chain_spec.blob_params_at_timestamp(input.meta.timestamp);
            receipts.push(build_receipt(&input, blob_params, |receipt_with_bloom| {
                ReceiptEnvelope::from_typed(tx_type, receipt_with_bloom)
            }));
        }

        Ok(receipts)
    }
}
