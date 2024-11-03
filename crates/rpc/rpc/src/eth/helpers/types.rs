//! L1 `eth` API types.

use alloy_consensus::{Signed, TxEip4844Variant, TxEnvelope};
use alloy_network::{Ethereum, Network};
use alloy_rpc_types::{Transaction, TransactionInfo};
use reth_primitives::{TransactionSigned, TransactionSignedEcRecovered};
use reth_rpc_types_compat::TransactionCompat;

/// Builds RPC transaction response for l1.
#[derive(Debug, Clone, Copy)]
pub struct EthTxBuilder;

impl TransactionCompat for EthTxBuilder
where
    Self: Send + Sync,
{
    type Transaction = <Ethereum as Network>::TransactionResponse;

    fn fill(
        &self,
        tx: TransactionSignedEcRecovered,
        tx_info: TransactionInfo,
    ) -> Self::Transaction {
        let from = tx.signer();
        let TransactionSigned { transaction, signature, hash } = tx.into_signed();

        let inner = match transaction {
            reth_primitives::Transaction::Legacy(tx) => {
                Signed::new_unchecked(tx, signature, hash).into()
            }
            reth_primitives::Transaction::Eip2930(tx) => {
                Signed::new_unchecked(tx, signature, hash).into()
            }
            reth_primitives::Transaction::Eip1559(tx) => {
                Signed::new_unchecked(tx, signature, hash).into()
            }
            reth_primitives::Transaction::Eip4844(tx) => {
                Signed::new_unchecked(tx, signature, hash).into()
            }
            reth_primitives::Transaction::Eip7702(tx) => {
                Signed::new_unchecked(tx, signature, hash).into()
            }
            #[allow(unreachable_patterns)]
            _ => unreachable!(),
        };

        let TransactionInfo { block_hash, block_number, index: transaction_index, .. } = tx_info;

        Transaction { inner, block_hash, block_number, transaction_index, from }
    }

    fn otterscan_api_truncate_input(tx: &mut Self::Transaction) {
        let input = match &mut tx.inner {
            TxEnvelope::Eip1559(tx) => &mut tx.tx_mut().input,
            TxEnvelope::Eip2930(tx) => &mut tx.tx_mut().input,
            TxEnvelope::Legacy(tx) => &mut tx.tx_mut().input,
            TxEnvelope::Eip4844(tx) => match tx.tx_mut() {
                TxEip4844Variant::TxEip4844(tx) => &mut tx.input,
                TxEip4844Variant::TxEip4844WithSidecar(tx) => &mut tx.tx.input,
            },
            TxEnvelope::Eip7702(tx) => &mut tx.tx_mut().input,
            _ => return,
        };
        *input = input.slice(..4);
    }
}
