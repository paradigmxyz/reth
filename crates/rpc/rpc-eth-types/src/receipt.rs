//! RPC receipt response builder, extends a layer one receipt with layer two data.

use crate::EthApiError;
use alloy_consensus::{ReceiptEnvelope, Transaction};
use alloy_eips::eip7840::BlobParams;
use alloy_primitives::{Address, TxKind, U256};
use alloy_rpc_types_eth::{Log, TransactionReceipt};
use reth_chainspec::EthChainSpec;
use reth_ethereum_primitives::Receipt;
use reth_evm::execute::frame_transaction_tip;
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

    let effective_gas_price = if let Some(frame) = tx.frame_transaction() {
        if gas_used == 0 {
            // Valid frames pay positive intrinsic gas; zero has no realized per-gas price.
            U256::ZERO
        } else {
            let base_fee = meta.base_fee.unwrap_or_default();
            let blob_charge = U256::from(blob_gas_used.unwrap_or_default()) *
                U256::from(blob_gas_price.unwrap_or_default());
            let tip = frame_transaction_tip(frame, gas_used, base_fee, blob_charge, None);
            // Blob fees have their own RPC fields; the execution price rounds down to whole wei.
            U256::from(base_fee) + tip / U256::from(gas_used)
        }
    } else {
        tx.effective_gas_price_u256(meta.base_fee)
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
        effective_gas_price,
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
    use alloy_consensus::{SignableTransaction, TxEip1559, TxEip8141, TxLegacy, TxType};
    use alloy_eips::{
        eip4844::DATA_GAS_PER_BLOB,
        eip8141::{
            Frame, FrameGasUsed, FrameLimits, FrameReceipt, FrameReceiptPayload, FrameStatus,
            TransactionFees, FRAME_TX_INTRINSIC_COST, FRAME_TX_PER_FRAME_COST,
        },
    };
    use alloy_primitives::{bytes, Sealable, Signature, B256};
    use reth_ethereum_primitives::{EthPrimitives, TransactionSigned};
    use reth_primitives_traits::Recovered;

    #[test]
    fn frame_receipt_retains_full_width_effective_gas_price() {
        let priority = U256::ONE << 180;
        let mut tx = frame_with_gas_limit(100_000);
        tx.fees = TransactionFees { max_fee: U256::ONE << 220, max_priority_fee_per_gas: priority };
        let rpc = frame_rpc_receipt(tx, 20_000, 10, None);
        assert_eq!(rpc.effective_gas_price, priority + U256::from(10));
        let json = serde_json::to_value(&rpc).unwrap();
        assert_eq!(
            json["effectiveGasPrice"],
            serde_json::to_value(priority + U256::from(10)).unwrap()
        );
    }

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
            let expected = ReceiptEnvelope::from(receipt.clone());
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

    #[test]
    fn frame_receipt_prices_realized_gas_with_capped_tip() {
        for (gas_used, expected) in [(20_000, 3), (80_000, 2), (100_000, 2)] {
            let tx = frame_with_gas_limit(100_000);
            assert_eq!(tx.gas_limit(), 100_000);
            assert_eq!(tx.effective_gas_price_u256(Some(1)), U256::from(2));
            let rpc = frame_rpc_receipt(tx, gas_used, 1, None);
            assert_eq!(rpc.gas_used, gas_used);
            assert_eq!(rpc.effective_gas_price, U256::from(expected));
        }
    }

    #[test]
    fn frame_receipt_reserves_blob_cost_without_including_it_in_execution_price() {
        let mut tx = frame_with_gas_limit(100_000);
        tx.blob_versioned_hashes = vec![B256::repeat_byte(1)];
        tx.fees.max_fee = U256::from(100_000 + DATA_GAS_PER_BLOB);
        let rpc = frame_rpc_receipt(tx, 50_000, 1, Some(BlobParams::cancun()));
        assert_eq!(rpc.blob_gas_used, Some(DATA_GAS_PER_BLOB));
        assert_eq!(rpc.blob_gas_price, Some(1));
        assert_eq!(rpc.effective_gas_price, U256::from(2));
    }

    #[test]
    fn frame_receipt_caps_overflowing_priority_products_to_signed_budget() {
        let mut tx = frame_with_gas_limit(100_000);
        tx.fees.max_fee = U256::MAX;
        tx.fees.max_priority_fee_per_gas = U256::MAX;
        let rpc = frame_rpc_receipt(tx, 20_000, 1, None);
        assert_eq!(rpc.effective_gas_price, U256::MAX / U256::from(20_000));
    }

    #[test]
    fn frame_receipt_with_no_gas_used_has_zero_effective_gas_price() {
        let rpc = frame_rpc_receipt(frame_with_gas_limit(100_000), 0, 1, None);
        assert_eq!(rpc.effective_gas_price, U256::ZERO);
    }

    #[test]
    fn ordinary_receipt_effective_gas_prices_are_unchanged() {
        for (tx, expected) in [
            (
                TransactionSigned::Legacy(
                    TxLegacy { gas_limit: 100_000, gas_price: 11, ..Default::default() }
                        .into_signed(Signature::test_signature()),
                ),
                11,
            ),
            (
                TransactionSigned::Eip1559(
                    TxEip1559 {
                        gas_limit: 100_000,
                        max_fee_per_gas: 10,
                        max_priority_fee_per_gas: 2,
                        ..Default::default()
                    }
                    .into_signed(Signature::test_signature()),
                ),
                9,
            ),
        ] {
            let converter = EthReceiptConverter::new(Arc::new(()));
            let rpc = build_receipt::<EthPrimitives, _>(
                ConvertReceiptInput {
                    receipt: Receipt::standard(tx.tx_type(), true, 21_000, vec![]),
                    tx: Recovered::new_unchecked(&tx, Address::repeat_byte(1)),
                    gas_used: 21_000,
                    next_log_index: 0,
                    meta: TransactionMeta { base_fee: Some(7), ..Default::default() },
                },
                None,
                &converter.build_rpc_receipt,
            );
            assert_eq!(rpc.effective_gas_price, U256::from(expected));
        }
    }

    fn frame_with_gas_limit(gas_limit: u64) -> TxEip8141 {
        TxEip8141 {
            frames: vec![Frame {
                limits: FrameLimits {
                    execution: gas_limit - FRAME_TX_INTRINSIC_COST - FRAME_TX_PER_FRAME_COST,
                    state: 0,
                },
                ..Default::default()
            }],
            fees: TransactionFees {
                max_fee: U256::from(200_000),
                max_priority_fee_per_gas: U256::from(2),
            },
            ..Default::default()
        }
    }

    fn frame_rpc_receipt(
        tx: TxEip8141,
        gas_used: u64,
        base_fee: u64,
        blob_params: Option<BlobParams>,
    ) -> TransactionReceipt {
        let sender = tx.sender;
        let tx = TransactionSigned::Eip8141(tx.seal_slow());
        let receipt = Receipt::from(ReceiptEnvelope::Eip8141(
            FrameReceiptPayload {
                cumulative_gas_used: gas_used,
                payer: sender,
                frame_receipts: vec![],
            }
            .into(),
        ));
        let converter = EthReceiptConverter::new(Arc::new(()));
        build_receipt::<EthPrimitives, _>(
            ConvertReceiptInput {
                receipt,
                tx: Recovered::new_unchecked(&tx, sender),
                gas_used,
                next_log_index: 0,
                meta: TransactionMeta {
                    base_fee: Some(base_fee),
                    excess_blob_gas: Some(0),
                    ..Default::default()
                },
            },
            blob_params,
            &converter.build_rpc_receipt,
        )
    }
}
