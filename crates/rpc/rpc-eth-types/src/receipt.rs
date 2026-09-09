//! RPC receipt response builder, extends a layer one receipt with layer two data.

use crate::EthApiError;
use alloy_consensus::{ReceiptEnvelope, Transaction};
use alloy_eips::eip7840::BlobParams;
use alloy_primitives::{Address, TxKind};
use alloy_rpc_types_eth::{Log, TransactionReceipt};
use reth_chainspec::EthChainSpec;
use reth_ethereum_primitives::Receipt;
use reth_primitives_traits::{NodePrimitives, SealedHeaderFor, TransactionMeta};
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
            build_rpc_receipt: |receipt: Receipt, next_log_index, meta: TransactionMeta| {
                if let Some(frame_receipt) = receipt.as_eip8141() {
                    tracing::info!(
                        target: "reth::eip8141::receipt",
                        tx_hash = ?meta.tx_hash,
                        block_hash = ?meta.block_hash,
                        block_number = meta.block_number,
                        transaction_index = meta.index,
                        payer = ?frame_receipt.payer,
                        frame_count = frame_receipt.frame_receipts.len(),
                        cumulative_gas_used = frame_receipt.cumulative_gas_used,
                        "converting EIP-8141 receipt for RPC"
                    );
                }
                let mut log_index = next_log_index;
                ReceiptEnvelope::from(receipt).map_logs(|log| {
                    let idx = log_index;
                    log_index += 1;
                    Log {
                        inner: log,
                        block_hash: Some(meta.block_hash),
                        block_number: Some(meta.block_number),
                        block_timestamp: Some(meta.timestamp),
                        transaction_hash: Some(meta.tx_hash),
                        transaction_index: Some(meta.index),
                        log_index: Some(idx as u64),
                        removed: false,
                    }
                })
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
    type RpcLog = Log;
    type Error = EthApiError;

    fn convert_log(
        &self,
        log: Log,
        _receipt: &N::Receipt,
        _header: &SealedHeaderFor<N>,
    ) -> Result<Self::RpcLog, Self::Error> {
        Ok(log)
    }

    fn convert_receipts(
        &self,
        inputs: Vec<ConvertReceiptInput<'_, N>>,
    ) -> Result<Vec<Self::RpcReceipt>, Self::Error> {
        let mut receipts = Vec::with_capacity(inputs.len());
        let blob_params = inputs
            .first()
            .and_then(|input| self.chain_spec.blob_params_at_timestamp(input.meta.timestamp));

        for input in inputs {
            receipts.push(build_receipt(input, blob_params, &self.build_rpc_receipt));
        }

        Ok(receipts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::TxType;
    use alloy_eips::eip8141::{FrameGasUsed, FrameReceipt, FrameReceiptPayload, FrameStatus};
    use alloy_primitives::{bytes, B256};

    #[test]
    fn owned_receipt_conversion_preserves_logs_and_rpc_indices() {
        let logs = vec![
            alloy_primitives::Log::new_unchecked(Address::repeat_byte(1), vec![], bytes!("01")),
            alloy_primitives::Log::new_unchecked(Address::repeat_byte(2), vec![], bytes!("02")),
        ];
        let standard = Receipt::standard(TxType::Eip1559, true, 42_000, logs.clone());
        let frame = Receipt::from(ReceiptEnvelope::Eip8141(
            FrameReceiptPayload {
                cumulative_gas_used: 42_000,
                payer: Address::repeat_byte(3),
                frame_receipts: logs
                    .iter()
                    .map(|log| FrameReceipt {
                        status: FrameStatus::Success,
                        gas_used: FrameGasUsed { execution: 21_000, state: 0 },
                        logs: vec![log.clone()],
                    })
                    .collect(),
            }
            .into(),
        ));
        let meta = TransactionMeta {
            tx_hash: B256::repeat_byte(4),
            block_hash: B256::repeat_byte(5),
            block_number: 12,
            timestamp: 34,
            index: 2,
            ..Default::default()
        };
        let converter = EthReceiptConverter::new(Arc::new(()));
        for receipt in [standard, frame] {
            let expected = receipt.to_envelope();
            let rpc = (converter.build_rpc_receipt)(receipt, 7, meta);
            assert_eq!(rpc.logs().len(), logs.len());
            for (index, log) in rpc.logs().iter().enumerate() {
                assert_eq!(log.inner, logs[index]);
                assert_eq!(log.log_index, Some(7 + index as u64));
                assert_eq!(log.transaction_index, Some(meta.index));
                assert_eq!(log.transaction_hash, Some(meta.tx_hash));
                assert_eq!(log.block_hash, Some(meta.block_hash));
                assert_eq!(log.block_number, Some(meta.block_number));
                assert_eq!(log.block_timestamp, Some(meta.timestamp));
                assert!(!log.removed);
            }
            assert_eq!(rpc.map_logs(|log| log.inner), expected);
        }
    }
}
