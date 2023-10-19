//!  Helper enum functions  for `Transaction`, `TransactionSigned` and
//! `TransactionSignedEcRecovered`
use reth_primitives::{Transaction, TransactionSigned, TransactionSignedEcRecovered};

/// Different transaction formats under one hood
#[derive(Debug, Clone)]
pub enum TransactionVariant {
    /// Transaction variant
    RegularTx(Transaction),
    /// TransactionSigned variant
    SignedTx(TransactionSigned),
    /// TransactionSignedEcRecovered variant
    SignedEcRecoveredTx(TransactionSignedEcRecovered),
}

impl TransactionVariant {
    /// Returns `Transaction` type
    /// else None
    pub fn as_regular_tx(&self) -> Option<&Transaction> {
        match self {
            TransactionVariant::RegularTx(tx) => Some(tx),
            _ => None,
        }
    }

    /// Returns `TransactionSigned` type
    /// else None
    pub fn as_signed_tx(&self) -> Option<&TransactionSigned> {
        match self {
            TransactionVariant::SignedTx(tx) => Some(tx),
            _ => None,
        }
    }

    /// Returns `TransactionSignedEcRecovered` type
    /// else None
    pub fn as_signed_ec_recovered_tx(&self) -> Option<&TransactionSignedEcRecovered> {
        match self {
            TransactionVariant::SignedEcRecoveredTx(tx) => Some(tx),
            _ => None,
        }
    }

    /// Returns true if the transaction is of `Transaction ` variant
    pub fn is_regular_tx(&self) -> bool {
        matches!(self, TransactionVariant::RegularTx(_))
    }

    /// Returns true if the transaction is of `TransactionSigned ` variant
    pub fn is_signed_tx(&self) -> bool {
        matches!(self, TransactionVariant::SignedTx(_))
    }

    /// Returns true if the transaction is of `TransactionSignedEcRecovered ` variant
    pub fn is_signed_ec_recovered_tx(&self) -> bool {
        matches!(self, TransactionVariant::SignedEcRecoveredTx(_))
    }

    /// Consumes the `TransactionVariant`
    /// Returns the consumed `Transaction` if its a `RegularTx` variant
    /// Otherwise returns None
    pub fn into_regular_tx(self) -> Option<Transaction> {
        match self {
            TransactionVariant::RegularTx(tx) => Some(tx),
            _ => None,
        }
    }

    /// Consumes the `TransactionVariant`
    /// Returns the consumed `TransactionSigned ` if its a `SignedTx` variant
    /// Otherwise returns None
    pub fn into_signed_tx(self) -> Option<TransactionSigned> {
        match self {
            TransactionVariant::SignedTx(tx) => Some(tx),
            _ => None,
        }
    }

    /// Consumes the `TransactionVariant`
    /// Returns the consumed `TransactionSignedEcRecovered ` if its a `SignedEcRecoveredTx` variant
    /// Otherwise returns None
    pub fn into_signed_ec_recovered_tx(self) -> Option<TransactionSignedEcRecovered> {
        match self {
            TransactionVariant::SignedEcRecoveredTx(tx) => Some(tx),
            _ => None,
        }
    }
}
