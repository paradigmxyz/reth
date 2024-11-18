//! Transaction abstraction

pub mod signed;

use core::{fmt, hash::Hash};

use alloy_primitives::B256;
use reth_codecs::Compact;

use crate::{FullTxType, InMemorySize, MaybeArbitrary, MaybeSerde, TxType};

/// Helper trait that unifies all behaviour required by transaction to support full node operations.
pub trait FullTransaction: Transaction<Type: FullTxType> + Compact {}

impl<T> FullTransaction for T where T: Transaction<Type: FullTxType> + Compact {}

/// Abstraction of a transaction.
pub trait Transaction:
    Send
    + Sync
    + Unpin
    + Clone
    + Default
    + fmt::Debug
    + Eq
    + PartialEq
    + Hash
    + TransactionExt
    + InMemorySize
    + MaybeSerde
    + MaybeArbitrary
{
}

impl<T> Transaction for T where
    T: Send
        + Sync
        + Unpin
        + Clone
        + Default
        + fmt::Debug
        + Eq
        + PartialEq
        + Hash
        + TransactionExt
        + InMemorySize
        + MaybeSerde
        + MaybeArbitrary
{
}

/// Extension trait of [`alloy_consensus::Transaction`].
#[auto_impl::auto_impl(&, Arc)]
pub trait TransactionExt: alloy_consensus::Transaction {
    /// Transaction envelope type ID.
    type Type: TxType;

    /// Heavy operation that return signature hash over rlp encoded transaction.
    /// It is only for signature signing or signer recovery.
    fn signature_hash(&self) -> B256;

    /// Returns the transaction type.
    fn tx_type(&self) -> Self::Type {
        Self::Type::try_from(self.ty()).expect("should decode tx type id")
    }
}
