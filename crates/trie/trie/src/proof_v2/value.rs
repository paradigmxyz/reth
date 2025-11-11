//! Generic value encoder types for proof calculation with lazy evaluation.

use crate::{
    hashed_cursor::HashedCursorFactory, proof_v2::ProofCalculator, trie_cursor::TrieCursorFactory,
};
use alloy_primitives::{B256, U256};
use alloy_rlp::Encodable;
use reth_execution_errors::trie::StateProofError;
use reth_primitives_traits::Account;
use reth_trie_common::RlpNode;
use std::rc::Rc;

/// A trait for deferred encoding of proof values.
///
/// This trait is implemented by types that can encode themselves into a buffer
/// on demand, allowing for lazy computation of storage roots and other deferred work.
pub trait ValueEncoderFut {
    /// RLP encodes the value into the provided buffer.
    ///
    /// # Arguments
    ///
    /// * `buf` - A mutable buffer to encode the data into
    fn encode(self, buf: &mut Vec<u8>) -> Result<(), StateProofError>;
}

/// A trait for encoding values for proof calculation.
///
/// This trait allows for lazy evaluation of encoding, enabling deferred
/// computation of storage roots and other expensive operations.
///
/// The encoder takes a reference to itself and a value, returning a future-like
/// type that will perform the encoding when needed.
pub trait ValueEncoder {
    /// The type of value being encoded (e.g., U256 for storage, Account for accounts).
    type Value;

    /// The type that will compute and encode the value when needed.
    type Fut: ValueEncoderFut;

    /// Returns a future-like value that will encode the value when called.
    ///
    /// # Arguments
    ///
    /// * `value` - The value to encode
    ///
    /// The returned future will be called as late as possible in the algorithm to maximize
    /// the time available for parallel computation (e.g., storage root calculation).
    fn encoder_fut(&self, value: Self::Value) -> Self::Fut;
}

/// An encoder for storage slot values.
///
/// This encoder simply RLP-encodes U256 storage values directly.
#[derive(Debug, Clone, Copy, Default)]
pub struct StorageValueEncoder;

/// The encoding future for a storage slot value.
#[derive(Debug, Clone, Copy)]
pub struct StorageValueEncoderFut(U256);

impl ValueEncoderFut for StorageValueEncoderFut {
    fn encode(self, buf: &mut Vec<u8>) -> Result<(), StateProofError> {
        buf.clear();
        self.0.encode(buf);
        Ok(())
    }
}

impl ValueEncoder for StorageValueEncoder {
    type Value = U256;
    type Fut = StorageValueEncoderFut;

    fn encoder_fut(&self, value: Self::Value) -> Self::Fut {
        StorageValueEncoderFut(value)
    }
}

/// An account value encoder that synchronously computes storage roots.
///
/// This encoder contains a provider that can create trie and hashed cursors.
/// Storage roots are computed lazily within the RLP encoding future by creating
/// a storage proof calculator on demand.
#[derive(Debug, Clone)]
pub struct SyncAccountValueEncoder<P> {
    /// Provider for creating trie and hashed cursors.
    provider: Rc<P>,
}

impl<P> SyncAccountValueEncoder<P> {
    /// Create a new account value encoder with the given provider.
    pub const fn new(provider: Rc<P>) -> Self {
        Self { provider }
    }
}

/// The encoding future for an account value with synchronous storage root calculation.
#[derive(Debug, Clone)]
pub struct SyncAccountValueEncoderFut<P> {
    provider: Rc<P>,
    hashed_address: B256,
    account: Account,
}

impl<P> ValueEncoderFut for SyncAccountValueEncoderFut<P>
where
    P: TrieCursorFactory + HashedCursorFactory,
{
    fn encode(self, buf: &mut Vec<u8>) -> Result<(), StateProofError> {
        // Create cursors for storage proof calculation
        let provider = &*self.provider;
        let trie_cursor = provider.storage_trie_cursor(self.hashed_address)?;
        let hashed_cursor = provider.hashed_storage_cursor(self.hashed_address)?;

        // Create storage proof calculator with StorageValueEncoder
        let mut storage_proof_calculator = ProofCalculator::new_storage(trie_cursor, hashed_cursor);

        // Compute storage root by calling storage_proof with empty targets
        // This returns just the root node of the storage trie
        let storage_root = storage_proof_calculator
            .storage_proof(self.hashed_address, std::iter::empty())
            .map(|nodes| {
                // Encode the root node to RLP
                let root_node =
                    nodes.first().expect("storage_proof always returns at least the root");
                buf.clear();
                root_node.node.encode(buf);

                // Hash the encoded node (RlpNode::from_rlp handles hashing for nodes >= 32 bytes)
                let rlp_node = RlpNode::from_rlp(buf);

                // Extract the hash - if it's a short node, it won't have a hash, so compute one
                rlp_node.as_hash().unwrap_or_else(|| alloy_primitives::keccak256(&*buf))
            })?;

        // Combine account with storage root to create TrieAccount
        let trie_account = self.account.into_trie_account(storage_root);

        // Encode the trie account
        buf.clear();
        trie_account.encode(buf);

        Ok(())
    }
}

impl<P> ValueEncoder for SyncAccountValueEncoder<P>
where
    P: TrieCursorFactory + HashedCursorFactory,
{
    type Value = (B256, Account);
    type Fut = SyncAccountValueEncoderFut<P>;

    fn encoder_fut(&self, value: Self::Value) -> Self::Fut {
        let (hashed_address, account) = value;

        // Return a future that will compute the storage root when encode() is called
        SyncAccountValueEncoderFut { provider: self.provider.clone(), hashed_address, account }
    }
}
