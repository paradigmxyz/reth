//!  Helper enum functions  for `Transaction`, `TransactionSigned` and
//! `TransactionSignedEcRecovered`
use crate::{Signature, Transaction, TransactionSigned, TransactionSignedEcRecovered};

/// Different transaction formats under one hood
#[derive(Debug, Clone)]
pub enum TransactionVariant {
    /// Transaction variant
    Plain(Transaction),
    /// TransactionSigned variant
    Signed(TransactionSigned),
    /// TransactionSignedEcRecovered variant
    SignedEcRecovered(TransactionSignedEcRecovered),
}

impl TransactionVariant {
    /// Returns `Transaction` type
    /// else None
    pub fn as_plain(&self) -> Option<&Transaction> {
        match self {
            TransactionVariant::Plain(tx) => Some(tx),
            _ => None,
        }
    }

    /// Returns `TransactionSigned` type
    /// else None
    pub fn as_signed(&self) -> Option<&TransactionSigned> {
        match self {
            TransactionVariant::Signed(tx) => Some(tx),
            _ => None,
        }
    }

    /// Returns `TransactionSignedEcRecovered` type
    /// else None
    pub fn as_signed_ec_recovered(&self) -> Option<&TransactionSignedEcRecovered> {
        match self {
            TransactionVariant::SignedEcRecovered(tx) => Some(tx),
            _ => None,
        }
    }

    /// Returns true if the transaction is of `Transaction ` variant
    pub fn is_plain(&self) -> bool {
        matches!(self, TransactionVariant::Plain(_))
    }

    /// Returns true if the transaction is of `TransactionSigned ` variant
    pub fn is_signed(&self) -> bool {
        matches!(self, TransactionVariant::Signed(_))
    }

    /// Returns true if the transaction is of `TransactionSignedEcRecovered ` variant
    pub fn is_signed_ec_recovered(&self) -> bool {
        matches!(self, TransactionVariant::SignedEcRecovered(_))
    }

    /// Consumes the `TransactionVariant`
    /// Returns the consumed `Transaction`
    pub fn into_plain(self) -> Option<Transaction> {
        match self {
            TransactionVariant::Plain(tx) => Some(tx),
            TransactionVariant::Signed(tx) => Some(tx.transaction),
            TransactionVariant::SignedEcRecovered(tx) => Some(tx.signed_transaction.transaction),
        }
    }

    /// Consumes the `TransactionVariant`
    /// Returns the consumed `TransactionSigned `
    pub fn into_signed(self, signature: Signature) -> Option<TransactionSigned> {
        match self {
            TransactionVariant::Signed(tx) => Some(tx),
            TransactionVariant::Plain(tx) => {
                let mut instance =
                    TransactionSigned { transaction: tx, signature, hash: Default::default() };
                instance.hash = instance.recalculate_hash();
                Some(instance)
            }
            TransactionVariant::SignedEcRecovered(tx) => Some(tx.signed_transaction),
        }
    }

    /// Consumes the `TransactionVariant`
    /// Returns the consumed `TransactionSignedEcRecovered `
    pub fn into_signed_ec_recovered(
        self,
        signature: Signature,
    ) -> Option<TransactionSignedEcRecovered> {
        match self {
            TransactionVariant::SignedEcRecovered(tx) => Some(tx),
            TransactionVariant::Signed(tx) => {
                let signer = tx.recover_signer();
                Some(TransactionSignedEcRecovered {
                    signer: signer.unwrap(),
                    signed_transaction: tx,
                })
            }
            TransactionVariant::Plain(tx) => {
                let mut instance =
                    TransactionSigned { transaction: tx, signature, hash: Default::default() };
                instance.hash = instance.recalculate_hash();
                let signer = instance.recover_signer();
                Some(TransactionSignedEcRecovered {
                    signer: signer.unwrap(),
                    signed_transaction: instance,
                })
            }
        }
    }
}
