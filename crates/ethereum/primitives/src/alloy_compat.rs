//! Common conversions from alloy types.

use crate::{Transaction, TransactionSigned};
use alloc::string::ToString;
use alloy_consensus::TxEnvelope;
use alloy_network::{AnyRpcTransaction, AnyTxEnvelope};
use alloy_serde::WithOtherFields;

impl TryFrom<AnyRpcTransaction> for TransactionSigned {
    type Error = alloy_rpc_types::ConversionError;

    fn try_from(tx: AnyRpcTransaction) -> Result<Self, Self::Error> {
        use alloy_rpc_types::ConversionError;

        let WithOtherFields { inner: tx, other: _ } = tx;

        #[allow(unreachable_patterns)]
        let (transaction, signature, hash) = match tx.inner {
            AnyTxEnvelope::Ethereum(TxEnvelope::Legacy(tx)) => {
                let (tx, signature, hash) = tx.into_parts();
                (Transaction::Legacy(tx), signature, hash)
            }
            AnyTxEnvelope::Ethereum(TxEnvelope::Eip2930(tx)) => {
                let (tx, signature, hash) = tx.into_parts();
                (Transaction::Eip2930(tx), signature, hash)
            }
            AnyTxEnvelope::Ethereum(TxEnvelope::Eip1559(tx)) => {
                let (tx, signature, hash) = tx.into_parts();
                (Transaction::Eip1559(tx), signature, hash)
            }
            AnyTxEnvelope::Ethereum(TxEnvelope::Eip4844(tx)) => {
                let (tx, signature, hash) = tx.into_parts();
                (Transaction::Eip4844(tx.into()), signature, hash)
            }
            AnyTxEnvelope::Ethereum(TxEnvelope::Eip7702(tx)) => {
                let (tx, signature, hash) = tx.into_parts();
                (Transaction::Eip7702(tx), signature, hash)
            }
            _ => return Err(ConversionError::Custom("unknown transaction type".to_string())),
        };

        Ok(Self { transaction, signature, hash: hash.into() })
    }
}
