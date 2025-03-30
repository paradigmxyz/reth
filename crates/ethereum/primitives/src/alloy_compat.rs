//! Common conversions from alloy types.


use crate::TransactionSigned;
use alloc::string::ToString;
use alloy_consensus::{
    EthereumTxEnvelope, TxEnvelope, TxLegacy, TxEip2930, TxEip1559, TxEip4844, TxEip7702,
};
use alloy_network::{AnyRpcTransaction, AnyTxEnvelope};
use alloy_rpc_types_eth::Transaction as AlloyRpcTransaction;

impl TryFrom<AnyRpcTransaction> for TransactionSigned {
    type Error = alloy_rpc_types_eth::ConversionError;

    fn try_from(tx: AnyRpcTransaction) -> Result<Self, Self::Error> {
        use alloy_rpc_types_eth::ConversionError;

        let tx = tx.into_inner();

        let (envelope, signature, hash) = match tx.inner.into_inner() {
            AnyTxEnvelope::Ethereum(TxEnvelope::Legacy(tx)) => {
                let (tx, signature, hash) = tx.into_parts();
                let envelope = EthereumTxEnvelope::<TxLegacy>::new(tx);
                (envelope, signature, hash)
            }
            AnyTxEnvelope::Ethereum(TxEnvelope::Eip2930(tx)) => {
                let (tx, signature, hash) = tx.into_parts();
                let envelope = EthereumTxEnvelope::<TxEip2930>::new(tx);
                (envelope, signature, hash)
            }
            AnyTxEnvelope::Ethereum(TxEnvelope::Eip1559(tx)) => {
                let (tx, signature, hash) = tx.into_parts();
                let envelope = EthereumTxEnvelope::<TxEip1559>::new(tx);
                (envelope, signature, hash)
            }
            AnyTxEnvelope::Ethereum(TxEnvelope::Eip4844(tx)) => {
                let (tx, signature, hash) = tx.into_parts();
                let envelope = EthereumTxEnvelope::<TxEip4844>::new(tx);
                (envelope, signature, hash)
            }
            AnyTxEnvelope::Ethereum(TxEnvelope::Eip7702(tx)) => {
                let (tx, signature, hash) = tx.into_parts();
                let envelope = EthereumTxEnvelope::<TxEip7702>::new(tx);
                (envelope, signature, hash)
            }
            _ => return Err(ConversionError::Custom("unknown transaction type".to_string())),
        };

        Ok(TransactionSigned::new(envelope, signature, hash))
    }
}

impl<T> From<AlloyRpcTransaction<T>> for TransactionSigned
where
    Self: From<T>,
{
    fn from(value: AlloyRpcTransaction<T>) -> Self {
        value.inner.into_inner().into()
    }
}
