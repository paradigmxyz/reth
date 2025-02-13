//! Common conversions from alloy types.

use crate::OpTransactionSigned;
use alloc::string::ToString;
use alloy_consensus::TxEnvelope;
use alloy_network::{AnyRpcTransaction, AnyTxEnvelope};
use alloy_rpc_types_eth::Transaction as AlloyRpcTransaction;
use alloy_serde::WithOtherFields;
use op_alloy_consensus::OpTypedTransaction;

impl TryFrom<AnyRpcTransaction> for OpTransactionSigned {
    type Error = alloy_rpc_types_eth::ConversionError;

    fn try_from(tx: AnyRpcTransaction) -> Result<Self, Self::Error> {
        use alloy_rpc_types_eth::ConversionError;

        let WithOtherFields { inner: tx, other: _ } = tx;

        let (transaction, signature, hash) = match tx.inner {
            AnyTxEnvelope::Ethereum(TxEnvelope::Legacy(tx)) => {
                let (tx, signature, hash) = tx.into_parts();
                (OpTypedTransaction::Legacy(tx), signature, hash)
            }
            AnyTxEnvelope::Ethereum(TxEnvelope::Eip2930(tx)) => {
                let (tx, signature, hash) = tx.into_parts();
                (OpTypedTransaction::Eip2930(tx), signature, hash)
            }
            AnyTxEnvelope::Ethereum(TxEnvelope::Eip1559(tx)) => {
                let (tx, signature, hash) = tx.into_parts();
                (OpTypedTransaction::Eip1559(tx), signature, hash)
            }
            AnyTxEnvelope::Ethereum(TxEnvelope::Eip7702(tx)) => {
                let (tx, signature, hash) = tx.into_parts();
                (OpTypedTransaction::Eip7702(tx), signature, hash)
            }
            _ => {
                // TODO: support tx deposit: <https://github.com/alloy-rs/op-alloy/pull/427>
                return Err(ConversionError::Custom("unknown transaction type".to_string()))
            }
        };

        Ok(Self::new(transaction, signature, hash))
    }
}

impl<T> From<AlloyRpcTransaction<T>> for OpTransactionSigned
where
    Self: From<T>,
{
    fn from(value: AlloyRpcTransaction<T>) -> Self {
        value.inner.into()
    }
}
