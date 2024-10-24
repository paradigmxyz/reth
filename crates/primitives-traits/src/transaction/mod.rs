use core::{fmt::Debug, hash::Hash};

use alloy_primitives::{TxKind, B256};

use alloy_rlp::Encodable;
use reth_codecs::Compact;
use serde::{Deserialize, Serialize};

pub mod signed;

#[allow(dead_code)]
/// Inner trait for a raw transaction.
trait TransactionInner<T>:
    Debug
    + Default
    + Clone
    + Eq
    + PartialEq
    + Encodable
    + Hash
    + Compact
    + Serialize
    + for<'de> Deserialize<'de>
    + From<T>
    + alloy_consensus::Transaction
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

/// Trait that defines methods of a raw transaction.
///
/// Transaction types were introduced in [EIP-2718](https://eips.ethereum.org/EIPS/eip-2718).
#[cfg(feature = "arbitrary")]
pub trait Transaction<T>: TransactionInner<T> + for<'b> arbitrary::Arbitrary<'b> {}

/// Trait that defines methods of a raw transaction.
///
/// Transaction types were introduced in [EIP-2718](https://eips.ethereum.org/EIPS/eip-2718).
#[cfg(not(feature = "arbitrary"))]
pub trait Transaction<T>: TransactionInner<T> {}
