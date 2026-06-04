//! Conversion helpers for feeding Reth Ethereum primitives into evm2.

use alloy_consensus::transaction::Recovered;
use evm2::ethereum::RecoveredTxEnvelope;
use reth_ethereum_primitives::TransactionSigned;

/// Converts an owned recovered Reth Ethereum transaction into evm2's recovered envelope.
pub fn evm2_recovered_tx(tx: Recovered<TransactionSigned>) -> RecoveredTxEnvelope {
    let (tx, signer) = tx.into_parts();
    match tx {
        TransactionSigned::Legacy(tx) => {
            RecoveredTxEnvelope::Legacy(Recovered::new_unchecked(tx, signer))
        }
        TransactionSigned::Eip2930(tx) => {
            RecoveredTxEnvelope::Eip2930(Recovered::new_unchecked(tx, signer))
        }
        TransactionSigned::Eip1559(tx) => {
            RecoveredTxEnvelope::Eip1559(Recovered::new_unchecked(tx, signer))
        }
        TransactionSigned::Eip4844(tx) => {
            RecoveredTxEnvelope::Eip4844(Recovered::new_unchecked(tx.into(), signer))
        }
        TransactionSigned::Eip7702(tx) => {
            RecoveredTxEnvelope::Eip7702(Recovered::new_unchecked(tx, signer))
        }
    }
}

/// Converts a borrowed recovered Reth Ethereum transaction into evm2's recovered envelope.
pub fn evm2_recovered_tx_ref(tx: Recovered<&TransactionSigned>) -> RecoveredTxEnvelope {
    evm2_recovered_tx(Recovered::new_unchecked(tx.inner().clone(), tx.signer()))
}
