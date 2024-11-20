//! Transaction abstraction

pub mod execute;
pub mod signed;

use core::{any, fmt, hash::Hash};

use alloy_consensus::constants::{
    EIP1559_TX_TYPE_ID, EIP2930_TX_TYPE_ID, EIP4844_TX_TYPE_ID, EIP7702_TX_TYPE_ID,
    LEGACY_TX_TYPE_ID,
};

use alloy_consensus::{TxEip1559, TxEip2930, TxEip4844, TxEip7702, TxLegacy};

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
    /// Returns true if the transaction is a legacy transaction.
    #[inline]
    fn is_legacy(&self) -> bool {
        matches!(self.ty(), LEGACY_TX_TYPE_ID)
    }

    /// Returns true if the transaction is an EIP-2930 transaction.
    #[inline]
    fn is_eip2930(&self) -> bool {
        matches!(self.ty(), EIP2930_TX_TYPE_ID)
    }

    /// Returns true if the transaction is an EIP-1559 transaction.
    #[inline]
    fn is_eip1559(&self) -> bool {
        matches!(self.ty(), EIP1559_TX_TYPE_ID)
    }

    /// Returns true if the transaction is an EIP-4844 transaction.
    #[inline]
    fn is_eip4844(&self) -> bool {
        matches!(self.ty(), EIP4844_TX_TYPE_ID)
    }

    /// Returns true if the transaction is an EIP-7702 transaction.
    #[inline]
    fn is_eip7702(&self) -> bool {
        matches!(self.ty(), EIP7702_TX_TYPE_ID)
    }

    /// Returns the [`TxLegacy`] variant if the transaction is a legacy transaction.
    fn as_legacy(&self) -> Option<&TxLegacy>;

    /// Returns the [`TxEip2930`] variant if the transaction is an EIP-2930 transaction.
    fn as_eip2930(&self) -> Option<&TxEip2930>;

    /// Returns the [`TxEip1559`] variant if the transaction is an EIP-1559 transaction.
    fn as_eip1559(&self) -> Option<&TxEip1559>;

    /// Returns the [`TxEip4844`] variant if the transaction is an EIP-4844 transaction.
    fn as_eip4844(&self) -> Option<&TxEip4844>;

    /// Returns the [`TxEip7702`] variant if the transaction is an EIP-7702 transaction.
    fn as_eip7702(&self) -> Option<&TxEip7702>;
}

impl<T> Transaction for T
where
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
        + MaybeArbitrary,
{
    fn as_legacy(&self) -> Option<&TxLegacy> {
        (self as &dyn any::Any).downcast_ref::<TxLegacy>().filter(|_| self.is_legacy())
    }

    fn as_eip2930(&self) -> Option<&TxEip2930> {
        (self as &dyn any::Any).downcast_ref::<TxEip2930>().filter(|_| self.is_eip2930())
    }

    fn as_eip1559(&self) -> Option<&TxEip1559> {
        (self as &dyn any::Any).downcast_ref::<TxEip1559>().filter(|_| self.is_eip1559())
    }

    fn as_eip4844(&self) -> Option<&TxEip4844> {
        (self as &dyn any::Any).downcast_ref::<TxEip4844>().filter(|_| self.is_eip4844())
    }

    fn as_eip7702(&self) -> Option<&TxEip7702> {
        (self as &dyn any::Any).downcast_ref::<TxEip7702>().filter(|_| self.is_eip7702())
    }
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
