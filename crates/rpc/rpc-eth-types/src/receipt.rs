//! RPC receipt response builder, extends a layer one receipt with layer two data.

use crate::EthApiError;
use alloy_consensus::{ReceiptEnvelope, Transaction};
use alloy_eips::eip7840::BlobParams;
use alloy_primitives::{Address, TxKind};
use alloy_rpc_types_eth::{Log, TransactionReceipt};
use reth_chainspec::EthChainSpec;
use reth_ethereum_primitives::Receipt;
use reth_primitives_traits::{NodePrimitives, TransactionMeta};
use reth_rpc_convert::transaction::{ConvertReceiptInput, ReceiptConverter};
use std::sync::Arc;

/// Builds an [`TransactionReceipt`] obtaining the inner receipt envelope from the given closure.
pub fn build_receipt<N, E>(
    input: ConvertReceiptInput<'_, N>,
    blob_params: Option<BlobParams>,
    build_rpc_receipt: impl FnOnce(N::Receipt, usize, TransactionMeta) -> E,
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
#[derive(derive_more::Debug)]
pub struct EthReceiptConverter<
    ChainSpec,
    Builder = fn(Receipt, usize, TransactionMeta) -> ReceiptEnvelope<Log>,
> {
    chain_spec: Arc<ChainSpec>,
    #[debug(skip)]
    build_rpc_receipt: Builder,
}

impl<ChainSpec, Builder> Clone for EthReceiptConverter<ChainSpec, Builder>
where
    Builder: Clone,
{
    fn clone(&self) -> Self {
        Self {
            chain_spec: self.chain_spec.clone(),
            build_rpc_receipt: self.build_rpc_receipt.clone(),
        }
    }
}

impl<ChainSpec> EthReceiptConverter<ChainSpec> {
    /// Creates a new converter with the given chain spec.
    pub const fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self {
            chain_spec,
            build_rpc_receipt: |receipt, next_log_index, meta| {
                receipt.into_rpc(next_log_index, meta).into()
            },
        }
    }

    /// Sets new builder for the converter.
    pub fn with_builder<Builder>(
        self,
        build_rpc_receipt: Builder,
    ) -> EthReceiptConverter<ChainSpec, Builder> {
        EthReceiptConverter { chain_spec: self.chain_spec, build_rpc_receipt }
    }
}

impl<N, ChainSpec, Builder, Rpc> ReceiptConverter<N> for EthReceiptConverter<ChainSpec, Builder>
where
    N: NodePrimitives,
    ChainSpec: EthChainSpec + 'static,
    Builder: Fn(N::Receipt, usize, TransactionMeta) -> Rpc + 'static,
{
    type RpcReceipt = TransactionReceipt<Rpc>;
    type Error = EthApiError;

    fn convert_receipts(
        &self,
        inputs: Vec<ConvertReceiptInput<'_, N>>,
    ) -> Result<Vec<Self::RpcReceipt>, Self::Error> {
        let mut receipts = Vec::with_capacity(inputs.len());

        for input in inputs {
            let blob_params = self.chain_spec.blob_params_at_timestamp(input.meta.timestamp);
            receipts.push(build_receipt(input, blob_params, &self.build_rpc_receipt));
        }

        Ok(receipts)
    }
}
