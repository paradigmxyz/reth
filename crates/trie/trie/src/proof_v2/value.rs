//! Generic value encoder types for proof calculation with lazy evaluation.

use crate::{
    hashed_cursor::{HashedCursorFactory, HashedStorageCursor},
    proof_v2::ProofCalculator,
    trie_cursor::{TrieCursorFactory, TrieStorageCursor},
};
use alloy_primitives::{B256, U256};
use alloy_rlp::Encodable;
use reth_execution_errors::trie::StateProofError;
use reth_primitives_traits::Account;
use reth_trie_common::RlpNode;

/// A trait for deferred RLP encoding of proof values.
///
/// This trait is implemented by types that can encode themselves into an RLP buffer
/// on demand, allowing for lazy computation of storage roots and other deferred work.
///
/// The lifetime `'a` ensures that the future cannot outlive the encoder that created it.
pub trait RlpNodeFut<'a>: 'a {
    /// Encodes the value as RLP into the provided buffer and returns the `RlpNode`.
    ///
    /// # Arguments
    ///
    /// * `buf` - A mutable buffer to encode the RLP data into
    ///
    /// # Returns
    ///
    /// The `RlpNode` representation of the encoded data
    fn get(self, buf: &mut Vec<u8>) -> Result<RlpNode, StateProofError>;
}

/// A trait for encoding values into RLP nodes for proof calculation.
///
/// This trait allows for lazy evaluation of RLP encoding, enabling deferred
/// computation of storage roots and other expensive operations.
///
/// The encoder takes a reference to itself and a value, returning a future-like
/// type that will perform the encoding when needed.
pub trait ValueEncoder {
    /// The type of value being encoded (e.g., U256 for storage, Account for accounts).
    type Value;

    /// The type that will compute and encode the RLP node when needed.
    ///
    /// The lifetime parameter ensures that the future cannot outlive the encoder.
    type RlpNodeFut<'a>: RlpNodeFut<'a>
    where
        Self: 'a;

    /// Returns a future-like value that will compute the RLP node when called.
    ///
    /// # Arguments
    ///
    /// * `value` - The value to encode
    ///
    /// The returned future should be called as late as possible in the algorithm to maximize
    /// the time available for parallel computation (e.g., storage root calculation).
    fn rlp_fut(&self, value: Self::Value) -> Self::RlpNodeFut<'_>;
}

/// An encoder for storage slot values.
///
/// This encoder simply RLP-encodes U256 storage values directly.
#[derive(Debug, Clone, Copy, Default)]
pub struct StorageValueEncoder;

/// The RLP encoding future for a storage slot value.
#[derive(Debug, Clone, Copy)]
pub struct StorageRlpNodeFut(U256);

impl<'a> RlpNodeFut<'a> for StorageRlpNodeFut {
    fn get(self, buf: &mut Vec<u8>) -> Result<RlpNode, StateProofError> {
        buf.clear();
        self.0.encode(buf);
        Ok(RlpNode::from_rlp(buf))
    }
}

impl ValueEncoder for StorageValueEncoder {
    type Value = U256;
    type RlpNodeFut<'a>
        = StorageRlpNodeFut
    where
        Self: 'a;

    fn rlp_fut(&self, value: Self::Value) -> Self::RlpNodeFut<'_> {
        StorageRlpNodeFut(value)
    }
}

/// An account value encoder that synchronously computes storage roots.
///
/// This encoder contains a reference to a provider that can create trie and hashed cursors.
/// Storage roots are computed lazily within the RLP encoding future by creating
/// a storage proof calculator on demand.
#[derive(Debug, Clone, Copy)]
pub struct SyncAccountValueEncoder<'a, P> {
    /// Provider for creating trie and hashed cursors.
    provider: &'a P,
}

impl<'a, P> SyncAccountValueEncoder<'a, P> {
    /// Create a new account value encoder with the given provider reference.
    pub const fn new(provider: &'a P) -> Self {
        Self { provider }
    }
}

/// The RLP encoding future for an account value with synchronous storage root calculation.
#[derive(Debug, Clone, Copy)]
pub struct SyncAccountRlpNodeFut<'a, P> {
    provider: &'a P,
    hashed_address: B256,
    account: Account,
}

impl<'fut, 'provider, P> RlpNodeFut<'fut> for SyncAccountRlpNodeFut<'provider, P>
where
    'provider: 'fut,
    P: TrieCursorFactory + HashedCursorFactory,
    P::StorageTrieCursor<'provider>: TrieStorageCursor,
    P::StorageCursor<'provider>: HashedStorageCursor,
{
    fn get(self, buf: &mut Vec<u8>) -> Result<RlpNode, StateProofError> {
        // Create cursors for storage proof calculation
        let trie_cursor = self.provider.storage_trie_cursor(self.hashed_address)?;
        let hashed_cursor = self.provider.hashed_storage_cursor(self.hashed_address)?;

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

        // Convert to RlpNode
        Ok(RlpNode::from_rlp(buf))
    }
}

impl<'provider, P> ValueEncoder for SyncAccountValueEncoder<'provider, P>
where
    P: TrieCursorFactory + HashedCursorFactory,
    P::StorageTrieCursor<'provider>: TrieStorageCursor,
    P::StorageCursor<'provider>: HashedStorageCursor,
{
    type Value = (B256, Account);
    type RlpNodeFut<'fut>
        = SyncAccountRlpNodeFut<'provider, P>
    where
        Self: 'fut,
        'provider: 'fut;

    fn rlp_fut(&self, value: Self::Value) -> Self::RlpNodeFut<'_> {
        let (hashed_address, account) = value;

        // Return a future that will compute the storage root when get() is called
        SyncAccountRlpNodeFut { provider: self.provider, hashed_address, account }
    }
}
