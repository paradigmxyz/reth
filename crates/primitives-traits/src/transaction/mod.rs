//! Transaction abstraction

use core::{fmt::Debug, hash::Hash};

use alloy_primitives::{TxKind, B256};

use reth_codecs::Compact;
use serde::{Deserialize, Serialize};

pub mod signed;

#[allow(dead_code)]
/// Abstraction of a transaction.
pub trait Transaction:
    Debug
    + Default
    + Clone
    + Eq
    + PartialEq
    + Hash
    + Serialize
    + alloy_rlp::Encodable
    + alloy_rlp::Decodable
    + for<'de> Deserialize<'de>
    + alloy_consensus::Transaction
    + MaybeArbitrary
{
    /// Heavy operation that return signature hash over rlp encoded transaction.
    /// It is only for signature signing or signer recovery.
    fn signature_hash(&self) -> B256;

    /// Gets the transaction's [`TxKind`], which is the address of the recipient or
    /// [`TxKind::Create`] if the transaction is a contract creation.
    fn kind(&self) -> TxKind;

    /// Returns true if the tx supports dynamic fees
    fn is_dynamic_fee(&self) -> bool;

    /// Returns the effective gas price for the given base fee.
    fn effective_gas_price(&self, base_fee: Option<u64>) -> u128;

    /// This encodes the transaction _without_ the signature, and is only suitable for creating a
    /// hash intended for signing.
    fn encode_without_signature(&self, out: &mut dyn bytes::BufMut);

    /// Calculates a heuristic for the in-memory size of the [Transaction].
    fn size(&self) -> usize;
}

#[cfg(not(feature = "arbitrary"))]
/// Helper trait that requires arbitrary implementation if the feature is enabled.
pub trait MaybeArbitrary {}

#[cfg(feature = "arbitrary")]
/// Helper trait that requires arbitrary implementation if the feature is enabled.
pub trait MaybeArbitrary: for<'a> arbitrary::Arbitrary<'a> {}

/// Helper trait that unifies all behaviour required by transaction to support full node operations.
pub trait FullTransaction: Transaction + Compact {}

impl<T> FullTransaction for T where T: Transaction + Compact {}
