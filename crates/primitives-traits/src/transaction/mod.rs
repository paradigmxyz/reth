//! Transaction abstraction

pub mod signed;

use core::{fmt, hash::Hash};

use alloy_primitives::B256;
use reth_codecs::Compact;
use serde::{Deserialize, Serialize};

use crate::{InMemorySize, MaybeArbitrary, TxType};

/// Helper trait that unifies all behaviour required by transaction to support full node operations.
pub trait FullTransaction: Transaction + Compact {}

impl<T> FullTransaction for T where T: Transaction + Compact {}

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
    + Serialize
    + for<'de> Deserialize<'de>
    + AlloyTransactionExt
    + InMemorySize
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
        + Serialize
        + for<'de> Deserialize<'de>
        + AlloyTransactionExt
        + InMemorySize
        + MaybeArbitrary
{
}

/// Extension trait of [`alloy_consensus::Transaction`].
pub trait AlloyTransactionExt: alloy_consensus::Transaction {
    /// Transaction envelope type ID.
    type Type: TxType;

    /// Heavy operation that return signature hash over rlp encoded transaction.
    /// It is only for signature signing or signer recovery.
    fn signature_hash(&self) -> B256;

    /// Returns `true` if the tx supports dynamic fees
    // todo: remove when released https://github.com/alloy-rs/alloy/pull/1638
    fn is_dynamic_fee(&self) -> bool;

    /// Returns the effective gas price for the given base fee.
    fn effective_gas_price(&self, base_fee: Option<u64>) -> u128;

    /// Returns the transaction type.
    fn tx_type(&self) -> Self::Type {
        Self::Type::try_from(self.ty()).expect("should decode tx type id")
    }
}
