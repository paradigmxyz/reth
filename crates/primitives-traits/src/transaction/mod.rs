use core::fmt::Debug;
use std::hash::Hash;

use alloy_primitives::{Bytes, TxKind, B256, U256};
use alloy_serde::WithOtherFields;

use alloy_consensus::{TxEip1559, TxEip2930, TxEip4844, TxEip7702, TxLegacy};
use alloy_rlp::Encodable;
use reth_codecs::Compact;
use serde::{Deserialize, Serialize};

pub mod signed;

/// Inner trait for a raw transaction.
///
/// Transaction types were introduced in [EIP-2718](https://eips.ethereum.org/EIPS/eip-2718).
pub trait TransactionInner<T>:
    Debug
    + Default
    + Clone
    + Eq
    + PartialEq
    + Encodable
    + Hash
    + Compact
    + Serialize
    + for<'a> Deserialize<'a>
    + derive_more::From<T>
    + alloy_consensus::Transaction
    + TryFrom<WithOtherFields<alloy_rpc_types::Transaction>>
{
    /// Heavy operation that return signature hash over rlp encoded transaction.
    /// It is only for signature signing or signer recovery.
    fn signature_hash(&self) -> B256;

    /// Sets the transaction's chain id to the provided value.
    fn set_chain_id(&mut self, chain_id: u64);

    /// Gets the transaction's [`TxKind`], which is the address of the recipient or
    /// [`TxKind::Create`] if the transaction is a contract creation.
    fn kind(&self) -> TxKind;

    /// Returns true if the tx supports dynamic fees
    fn is_dynamic_fee(&self) -> bool;

    /// Returns the blob gas used for all blobs of the EIP-4844 transaction if it is an EIP-4844
    /// transaction.
    ///
    /// This is the number of blobs times the
    /// [`DATA_GAS_PER_BLOB`](crate::constants::eip4844::DATA_GAS_PER_BLOB) a single blob consumes.
    fn blob_gas_used(&self) -> Option<u64>;

    /// Returns the effective gas price for the given base fee.
    ///
    /// If the transaction is a legacy or EIP2930 transaction, the gas price is returned.
    fn effective_gas_price(&self, base_fee: Option<u64>) -> u128;

    /// This encodes the transaction _without_ the signature, and is only suitable for creating a
    /// hash intended for signing.
    fn encode_without_signature(&self, out: &mut dyn bytes::BufMut);

    /// This sets the transaction's gas limit.
    fn set_gas_limit(&mut self, gas_limit: u64);

    /// This sets the transaction's nonce.
    fn set_nonce(&mut self, nonce: u64);

    /// This sets the transaction's value.
    fn set_value(&mut self, value: U256);

    /// This sets the transaction's input field.
    fn set_input(&mut self, input: Bytes);

    /// Calculates a heuristic for the in-memory size of the [Transaction].
    fn size(&self) -> usize;

    /// Returns true if the transaction is a legacy transaction.
    fn is_legacy(&self) -> bool;

    /// Returns true if the transaction is an EIP-2930 transaction.
    fn is_eip2930(&self) -> bool;

    /// Returns true if the transaction is an EIP-1559 transaction.
    fn is_eip1559(&self) -> bool;

    /// Returns true if the transaction is an EIP-4844 transaction.
    fn is_eip4844(&self) -> bool;

    /// Returns true if the transaction is an EIP-7702 transaction.
    fn is_eip7702(&self) -> bool;

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

    /// Returns the source hash of the transaction, which uniquely identifies its source.
    /// If not a deposit transaction, this will always return `None`.
    #[cfg(feature = "optimism")]
    fn source_hash(&self) -> Option<B256>;

    /// Returns the amount of ETH locked up on L1 that will be minted on L2. If the transaction
    /// is not a deposit transaction, this will always return `None`.
    #[cfg(feature = "optimism")]
    fn mint(&self) -> Option<u128>;

    /// Returns whether or not the transaction is a system transaction. If the transaction
    /// is not a deposit transaction, this will always return `false`.
    #[cfg(feature = "optimism")]
    fn is_system_transaction(&self) -> bool;

    /// Returns whether or not the transaction is an Optimism Deposited transaction.
    #[cfg(feature = "optimism")]
    fn is_deposit(&self) -> bool;
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
