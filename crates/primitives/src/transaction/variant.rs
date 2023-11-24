//!  Helper enum functions  for `Transaction`, `TransactionSigned` and
//! `TransactionSignedEcRecovered`
use crate::{
    Address, Transaction, TransactionSigned, TransactionSignedEcRecovered, TransactionSignedNoHash,
    B256,
};
use std::ops::Deref;

/// Represents various different transaction formats used in reth.
///
/// All variants are based on a the raw [Transaction] data and can contain additional information
/// extracted (expensive) from that transaction, like the hash and the signer.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TransactionSignedVariant {
    /// A signed transaction without a hash.
    SignedNoHash(TransactionSignedNoHash),
    /// Contains the plain transaction data its signature and hash.
    Signed(TransactionSigned),
    /// Contains the plain transaction data its signature and hash and the successfully recovered
    /// signer.
    SignedEcRecovered(TransactionSignedEcRecovered),
}

impl TransactionSignedVariant {
    /// Returns the raw transaction object
    pub fn as_raw(&self) -> &Transaction {
        match self {
            TransactionSignedVariant::SignedNoHash(tx) => &tx.transaction,
            TransactionSignedVariant::Signed(tx) => &tx.transaction,
            TransactionSignedVariant::SignedEcRecovered(tx) => &tx.signed_transaction.transaction,
        }
    }

    /// Returns the hash of the transaction
    pub fn hash(&self) -> B256 {
        match self {
            TransactionSignedVariant::SignedNoHash(tx) => tx.hash(),
            TransactionSignedVariant::Signed(tx) => tx.hash,
            TransactionSignedVariant::SignedEcRecovered(tx) => tx.hash,
        }
    }

    /// Returns the signer of the transaction.
    ///
    /// If the transaction is of not of [TransactionSignedEcRecovered] it will be recovered.
    pub fn signer(&self) -> Option<Address> {
        match self {
            TransactionSignedVariant::SignedNoHash(tx) => tx.recover_signer(),
            TransactionSignedVariant::Signed(tx) => tx.recover_signer(),
            TransactionSignedVariant::SignedEcRecovered(tx) => Some(tx.signer),
        }
    }

    /// Returns [TransactionSigned] type
    /// else None
    pub fn as_signed(&self) -> Option<&TransactionSigned> {
        match self {
            TransactionSignedVariant::Signed(tx) => Some(tx),
            _ => None,
        }
    }

    /// Returns `TransactionSignedEcRecovered` type
    /// else None
    pub fn as_signed_ec_recovered(&self) -> Option<&TransactionSignedEcRecovered> {
        match self {
            TransactionSignedVariant::SignedEcRecovered(tx) => Some(tx),
            _ => None,
        }
    }

    /// Returns true if the transaction is of [TransactionSigned] variant
    pub fn is_signed(&self) -> bool {
        matches!(self, TransactionSignedVariant::Signed(_))
    }

    /// Returns true if the transaction is of [TransactionSignedNoHash] variant
    pub fn is_signed_no_hash(&self) -> bool {
        matches!(self, TransactionSignedVariant::SignedNoHash(_))
    }

    /// Returns true if the transaction is of [TransactionSignedEcRecovered] variant
    pub fn is_signed_ec_recovered(&self) -> bool {
        matches!(self, TransactionSignedVariant::SignedEcRecovered(_))
    }

    /// Consumes the [TransactionSignedVariant] and returns the consumed [Transaction]
    pub fn into_raw(self) -> Transaction {
        match self {
            TransactionSignedVariant::SignedNoHash(tx) => tx.transaction,
            TransactionSignedVariant::Signed(tx) => tx.transaction,
            TransactionSignedVariant::SignedEcRecovered(tx) => tx.signed_transaction.transaction,
        }
    }

    /// Consumes the [TransactionSignedVariant] and returns the consumed [TransactionSigned]
    pub fn into_signed(self) -> TransactionSigned {
        match self {
            TransactionSignedVariant::SignedNoHash(tx) => tx.with_hash(),
            TransactionSignedVariant::Signed(tx) => tx,
            TransactionSignedVariant::SignedEcRecovered(tx) => tx.signed_transaction,
        }
    }

    /// Consumes the [TransactionSignedVariant] and converts it into a
    /// [TransactionSignedEcRecovered]
    ///
    /// If the variants is not a [TransactionSignedEcRecovered] it will recover the sender.
    ///
    /// Returns `None` if the transaction's signature is invalid
    pub fn into_signed_ec_recovered(self) -> Option<TransactionSignedEcRecovered> {
        self.try_into_signed_ec_recovered().ok()
    }

    /// Consumes the [TransactionSignedVariant] and converts it into a
    /// [TransactionSignedEcRecovered]
    ///
    /// If the variants is not a [TransactionSignedEcRecovered] it will recover the sender.
    ///
    /// Returns an error if the transaction's signature is invalid.
    pub fn try_into_signed_ec_recovered(
        self,
    ) -> Result<TransactionSignedEcRecovered, TransactionSigned> {
        match self {
            TransactionSignedVariant::SignedEcRecovered(tx) => Ok(tx),
            TransactionSignedVariant::Signed(tx) => tx.try_into_ecrecovered(),
            TransactionSignedVariant::SignedNoHash(tx) => tx.with_hash().try_into_ecrecovered(),
        }
    }
}

impl From<TransactionSignedNoHash> for TransactionSignedVariant {
    fn from(tx: TransactionSignedNoHash) -> Self {
        TransactionSignedVariant::SignedNoHash(tx)
    }
}

impl From<TransactionSigned> for TransactionSignedVariant {
    fn from(tx: TransactionSigned) -> Self {
        TransactionSignedVariant::Signed(tx)
    }
}

impl From<TransactionSignedEcRecovered> for TransactionSignedVariant {
    fn from(tx: TransactionSignedEcRecovered) -> Self {
        TransactionSignedVariant::SignedEcRecovered(tx)
    }
}

impl AsRef<Transaction> for TransactionSignedVariant {
    fn as_ref(&self) -> &Transaction {
        self.as_raw()
    }
}

impl Deref for TransactionSignedVariant {
    type Target = Transaction;

    fn deref(&self) -> &Self::Target {
        self.as_raw()
    }
}
