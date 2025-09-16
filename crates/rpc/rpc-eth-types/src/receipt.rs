//! RPC receipt response builder, extends a layer one receipt with layer two data.

use crate::EthApiError;
use alloy_consensus::{Transaction, TxReceipt};
use alloy_eips::eip7840::BlobParams;
use alloy_primitives::{Address, TxKind};
use alloy_rpc_types_eth::{ReceiptWithBloom, TransactionReceipt};
use reth_chainspec::EthChainSpec;
use reth_ethereum_primitives::{Receipt, TxTy};
use reth_primitives_traits::{NodePrimitives, TransactionMeta};
use reth_rpc_convert::transaction::{ConvertReceiptInput, ReceiptConverter};
use std::sync::Arc;

/// Builds an [`TransactionReceipt`] obtaining the inner receipt envelope from the given closure.
pub fn build_receipt<N, E>(
    input: ConvertReceiptInput<'_, N>,
    blob_params: Option<BlobParams>,
    build_rpc_receipt: impl FnOnce(ReceiptWithBloom<N::Receipt>, usize, TransactionMeta) -> E,
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

    let receipt = receipt.into_with_bloom();

    let (contract_address, to) = match tx.kind() {
        TxKind::Create => (Some(from.create(tx.nonce())), None),
        TxKind::Call(addr) => (None, Some(Address(*addr))),
    };

    TransactionReceipt {
        inner: build_rpc_receipt(receipt, next_log_index, meta),
        transaction_hash: meta.tx_hash,
        transaction_index: Some(meta.index),
        block_hash: Some(meta.block_hash),
        block_number: Some(meta.block_number),
        from,
        to,
        gas_used,
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

impl<N, ChainSpec, T> ReceiptConverter<N> for EthReceiptConverter<ChainSpec>
where
    N: NodePrimitives<Receipt = Receipt<T>>,
    ChainSpec: EthChainSpec + 'static,
    T: TxTy,
{
    type RpcReceipt = TransactionReceipt<ReceiptWithBloom<Receipt<T, alloy_rpc_types_eth::Log>>>;
    type Error = EthApiError;

    fn convert_receipts(
        &self,
        inputs: Vec<ConvertReceiptInput<'_, N>>,
    ) -> Result<Vec<Self::RpcReceipt>, Self::Error> {
        let mut receipts = Vec::with_capacity(inputs.len());

        for input in inputs {
            let blob_params = self.chain_spec.blob_params_at_timestamp(input.meta.timestamp);
            receipts.push(build_receipt(input, blob_params, |receipt, next_log_index, meta| {
                receipt.map_receipt(|receipt| receipt.into_rpc_logs(next_log_index, meta))
            }));
        }

        Ok(receipts)
    }
}
