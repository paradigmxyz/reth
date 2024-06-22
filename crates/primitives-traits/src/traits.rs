//! Traits for primitive types.

use alloy_consensus::Sealed;
use alloy_primitives::{ChainId, Sealable, TxKind, B256, U256};
use core::fmt;
use std::any;

/// A type responsible for storing and retrieving data from the given database.
pub trait Storage<N: NodePrimitives> {
    /// Error that can occur when interacting with the storage.
    type Error;

    /// Writes a sealed block to the storage.
    fn write_sealed_block(&self, block: Sealed<N::Block>) -> Result<(), Self::Error>;
}

/// The network's primitive types.
pub trait NodePrimitives: Send + Sync + fmt::Debug + 'static {
    /// The block type for the network.
    type Block: Block;
    /// The receipt type for the network.
    type Receipt: Receipt;
}

/// The header type for a block.
pub trait Header:
    Sealable + Sized + Clone + Send + Sync + Eq + PartialEq + fmt::Debug + 'static
{
    /// Returns the parent block's hash.
    fn parent_hash(&self) -> &B256;

    // /// Return's the receipt root of the block.
    // fn receipt_root(&self) -> &B256;

    // /// Returns the block's transactions root.
    // fn transactions_root(&self) -> &B256;

    /// Returns the block's number.
    fn block_number(&self) -> u64;

    /// Returns the block's timestamp.
    fn timestamp(&self) -> u64;
}

/// The raw block type.
pub trait Block:
    Sealable + Sized + Clone + Send + Sync + Eq + PartialEq + fmt::Debug + 'static
{
    /// The block's header type.
    type Header: Header;

    /// The block's body, including transactions.
    type Body: BlockBody;

    /// Returns the block's header.
    fn header(&self) -> &Self::Header;

    /// Returns the block's body.
    fn body(&self) -> &Self::Body;
}

/// Represents the body of a block.
pub trait BlockBody: Sized + Clone + Send + Sync + Eq + PartialEq + fmt::Debug + 'static {
    /// The transaction type of the block.
    type Transaction: Transaction;

    /// Returns the block's transactions.
    fn transactions(&self) -> &[Self::Transaction];
}

/// A raw transaction type.
// TODO replace with alloy trait?
pub trait Transaction:
    Sized + any::Any + Clone + Send + Sync + Eq + PartialEq + fmt::Debug + 'static
{
    /// Get `chain_id`.
    fn chain_id(&self) -> Option<ChainId>;

    /// Get `nonce`.
    fn nonce(&self) -> u64;

    /// Get `gas_limit`.
    fn gas_limit(&self) -> u128;

    /// Get `gas_price`.
    fn gas_price(&self) -> Option<u128>;

    /// Get `to`.
    fn to(&self) -> TxKind;

    /// Get `value`.
    fn value(&self) -> U256;

    /// Get `data`.
    fn input(&self) -> &[u8];
}

/// The network's receipt type.
pub trait Receipt:
    Sized + any::Any + Clone + Send + Sync + Eq + PartialEq + fmt::Debug + 'static
{
}
