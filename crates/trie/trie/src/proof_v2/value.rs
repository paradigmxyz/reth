//! Generic value encoder types for proof calculation with lazy evaluation.

use crate::{
    hashed_cursor::HashedCursorFactory, proof_v2::ProofCalculator, trie_cursor::TrieCursorFactory,
};
use alloy_primitives::{B256, U256};
use alloy_rlp::Encodable;
use reth_execution_errors::trie::StateProofError;
use reth_primitives_traits::Account;
use reth_trie_common::Nibbles;
use std::rc::Rc;

/// A trait for deferred RLP-encoding of leaf values.
pub trait DeferredValueEncoder {
    /// RLP encodes the value into the provided buffer.
    ///
    /// # Arguments
    ///
    /// * `buf` - A mutable buffer to encode the data into
    fn encode(self, buf: &mut Vec<u8>) -> Result<(), StateProofError>;
}

/// A trait for RLP-encoding values for proof calculation. This trait is designed to allow the lazy
/// computation of leaf values in a generic way.
///
/// When calculating a leaf value in a storage trie the [`DeferredValueEncoder`] simply holds onto
/// the slot value, and the `encode` method synchronously RLP-encodes it.
///
/// When calculating a leaf value in the accounts trie we create a [`DeferredValueEncoder`] to
/// initiate any asynchronous computation of the account's storage root we want to do. Later we call
/// [`DeferredValueEncoder::encode`] to obtain the result of that computation and RLP-encode it.
pub trait LeafValueEncoder {
    /// The type of value being encoded (e.g., U256 for storage, Account for accounts).
    type Value;

    /// The type that will compute and encode the value when needed.
    type DeferredEncoder: DeferredValueEncoder;

    /// Returns an encoder that will RLP-encode the value when its `encode` method is called.
    ///
    /// # Arguments
    ///
    /// * `key` - The key the value was stored at in the DB
    /// * `value` - The value to encode
    ///
    /// The returned deferred encoder will be called as late as possible in the algorithm to
    /// maximize the time available for parallel computation (e.g., storage root calculation).
    fn deferred_encoder(&self, key: B256, value: Self::Value) -> Self::DeferredEncoder;
}

/// An encoder for storage slot values.
///
/// This encoder simply RLP-encodes U256 storage values directly.
#[derive(Debug, Clone, Copy, Default)]
pub struct StorageValueEncoder;

/// The deferred encoder for a storage slot value.
#[derive(Debug, Clone, Copy)]
pub struct StorageDeferredValueEncoder(U256);

impl DeferredValueEncoder for StorageDeferredValueEncoder {
    fn encode(self, buf: &mut Vec<u8>) -> Result<(), StateProofError> {
        self.0.encode(buf);
        Ok(())
    }
}

impl LeafValueEncoder for StorageValueEncoder {
    type Value = U256;
    type DeferredEncoder = StorageDeferredValueEncoder;

    fn deferred_encoder(&self, _key: B256, value: Self::Value) -> Self::DeferredEncoder {
        StorageDeferredValueEncoder(value)
    }
}

/// An account value encoder that synchronously computes storage roots.
///
/// This encoder contains a provider that can create trie and hashed cursors. Storage roots are
/// computed synchronously within the deferred encoder using a `StorageProofCalculator`.
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

/// The deferred encoder for an account value with synchronous storage root calculation.
#[derive(Debug, Clone)]
pub struct SyncAccountDeferredValueEncoder<P> {
    provider: Rc<P>,
    hashed_address: B256,
    account: Account,
}

impl<P> DeferredValueEncoder for SyncAccountDeferredValueEncoder<P>
where
    P: TrieCursorFactory + HashedCursorFactory,
{
    // Synchronously computes the storage root for this account and RLP-encodes the resulting
    // `TrieAccount` into `buf`
    fn encode(self, buf: &mut Vec<u8>) -> Result<(), StateProofError> {
        // Create cursors for storage proof calculation
        let provider = &*self.provider;
        let trie_cursor = provider.storage_trie_cursor(self.hashed_address)?;
        let hashed_cursor = provider.hashed_storage_cursor(self.hashed_address)?;

        // Create storage proof calculator with StorageValueEncoder
        let mut storage_proof_calculator = ProofCalculator::new_storage(trie_cursor, hashed_cursor);

        // Compute storage root by calling storage_proof with the root path as a target.
        // This returns just the root node of the storage trie.
        let storage_root = storage_proof_calculator
            .storage_proof(self.hashed_address, [Nibbles::new()])
            .map(|nodes| {
                // Encode the root node to RLP and hash it
                let root_node =
                    nodes.first().expect("storage_proof always returns at least the root");
                root_node.node.encode(buf);

                let storage_root = alloy_primitives::keccak256(buf.as_slice());

                // Clear the buffer so we can re-use it to encode the TrieAccount
                buf.clear();

                storage_root
            })?;

        // Combine account with storage root to create TrieAccount
        let trie_account = self.account.into_trie_account(storage_root);

        // Encode the trie account
        trie_account.encode(buf);

        Ok(())
    }
}

impl<P> LeafValueEncoder for SyncAccountValueEncoder<P>
where
    P: TrieCursorFactory + HashedCursorFactory,
{
    type Value = Account;
    type DeferredEncoder = SyncAccountDeferredValueEncoder<P>;

    fn deferred_encoder(
        &self,
        hashed_address: B256,
        account: Self::Value,
    ) -> Self::DeferredEncoder {
        // Return a deferred encoder that will synchronously compute the storage root when encode()
        // is called.
        SyncAccountDeferredValueEncoder { provider: self.provider.clone(), hashed_address, account }
    }
}
