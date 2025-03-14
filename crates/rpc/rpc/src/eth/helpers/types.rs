//! L1 `eth` API types.

use alloy_consensus::{Transaction as _, TxEip4844Variant, TxEnvelope};
use alloy_network::{Ethereum, Network};
use alloy_primitives::PrimitiveSignature as Signature;
use alloy_rpc_types::TransactionRequest;
use alloy_rpc_types_eth::{Transaction, TransactionInfo};
use reth_ethereum_primitives::TransactionSigned;
use reth_primitives_traits::Recovered;
use reth_rpc_eth_api::EthApiTypes;
use reth_rpc_eth_types::EthApiError;
use reth_rpc_types_compat::TransactionCompat;

/// A standalone [`EthApiTypes`] implementation for Ethereum.
#[derive(Debug, Clone, Copy, Default)]
pub struct EthereumEthApiTypes(EthTxBuilder);

impl EthApiTypes for EthereumEthApiTypes {
    type Error = EthApiError;
    type NetworkTypes = Ethereum;
    type TransactionCompat = EthTxBuilder;

    fn tx_resp_builder(&self) -> &Self::TransactionCompat {
        &self.0
    }
}

/// Builds RPC transaction response for l1.
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct EthTxBuilder;

impl TransactionCompat<TransactionSigned> for EthTxBuilder
where
    Self: Send + Sync,
{
    type Transaction = <Ethereum as Network>::TransactionResponse;

    type Error = EthApiError;

    fn fill(
        &self,
        tx: Recovered<TransactionSigned>,
        tx_info: TransactionInfo,
    ) -> Result<Self::Transaction, Self::Error> {
        let tx = tx.convert::<TxEnvelope>();

        let TransactionInfo {
            block_hash, block_number, index: transaction_index, base_fee, ..
        } = tx_info;

        let effective_gas_price = base_fee
            .map(|base_fee| {
                tx.effective_tip_per_gas(base_fee).unwrap_or_default() + base_fee as u128
            })
            .unwrap_or_else(|| tx.max_fee_per_gas());

        Ok(Transaction {
            inner: tx,
            block_hash,
            block_number,
            transaction_index,
            effective_gas_price: Some(effective_gas_price),
        })
    }

    fn build_simulate_v1_transaction(
        &self,
        request: TransactionRequest,
    ) -> Result<TransactionSigned, Self::Error> {
        let Ok(tx) = request.build_typed_tx() else {
            return Err(EthApiError::TransactionConversionError)
        };

        // Create an empty signature for the transaction.
        let signature = Signature::new(Default::default(), Default::default(), false);
        Ok(TransactionSigned::new_unhashed(tx.into(), signature))
    }

    fn otterscan_api_truncate_input(tx: &mut Self::Transaction) {
        let input = match tx.inner.inner_mut() {
            TxEnvelope::Eip1559(tx) => &mut tx.tx_mut().input,
            TxEnvelope::Eip2930(tx) => &mut tx.tx_mut().input,
            TxEnvelope::Legacy(tx) => &mut tx.tx_mut().input,
            TxEnvelope::Eip4844(tx) => match tx.tx_mut() {
                TxEip4844Variant::TxEip4844(tx) => &mut tx.input,
                TxEip4844Variant::TxEip4844WithSidecar(tx) => &mut tx.tx.input,
            },
            TxEnvelope::Eip7702(tx) => &mut tx.tx_mut().input,
        };
        *input = input.slice(..4);
    }
}
