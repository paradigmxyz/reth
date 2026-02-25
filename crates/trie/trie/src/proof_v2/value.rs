//! Generic value encoder types for proof calculation with lazy evaluation.

use crate::{
    hashed_cursor::HashedCursorFactory, proof_v2::ProofCalculator, trie_cursor::TrieCursorFactory,
};
use alloy_primitives::{B256, U256};
use alloy_rlp::Encodable;
use reth_execution_errors::trie::StateProofError;
use reth_primitives_traits::Account;
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
    fn deferred_encoder(&mut self, key: B256, value: Self::Value) -> Self::DeferredEncoder;
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

    fn deferred_encoder(&mut self, _key: B256, value: Self::Value) -> Self::DeferredEncoder {
        StorageDeferredValueEncoder(value)
    }
}

/// An account value encoder that synchronously computes storage roots.
///
/// This encoder contains factories for creating trie and hashed cursors. Storage roots are
/// computed synchronously within the deferred encoder using a `StorageProofCalculator`.
#[derive(Debug, Clone)]
pub struct SyncAccountValueEncoder<T, H> {
    /// Factory for creating trie cursors.
    trie_cursor_factory: Rc<T>,
    /// Factory for creating hashed cursors.
    hashed_cursor_factory: Rc<H>,
}

impl<T, H> SyncAccountValueEncoder<T, H> {
    /// Create a new account value encoder with the given factories.
    pub fn new(trie_cursor_factory: T, hashed_cursor_factory: H) -> Self {
        Self {
            trie_cursor_factory: Rc::new(trie_cursor_factory),
            hashed_cursor_factory: Rc::new(hashed_cursor_factory),
        }
    }
}

/// The deferred encoder for an account value with synchronous storage root calculation.
#[derive(Debug, Clone)]
pub struct SyncAccountDeferredValueEncoder<T, H> {
    trie_cursor_factory: Rc<T>,
    hashed_cursor_factory: Rc<H>,
    hashed_address: B256,
    account: Account,
}

impl<T, H> DeferredValueEncoder for SyncAccountDeferredValueEncoder<T, H>
where
    T: TrieCursorFactory,
    H: HashedCursorFactory,
{
    fn encode(self, buf: &mut Vec<u8>) -> Result<(), StateProofError> {
        let trie_cursor = self.trie_cursor_factory.storage_trie_cursor(self.hashed_address)?;
        let hashed_cursor =
            self.hashed_cursor_factory.hashed_storage_cursor(self.hashed_address)?;

        let mut storage_proof_calculator = ProofCalculator::new_storage(trie_cursor, hashed_cursor);
        let root_node = storage_proof_calculator.storage_root_node(self.hashed_address)?;
        let storage_root = storage_proof_calculator
            .compute_root_hash(&[root_node])?
            .expect("storage_root_node returns a node at empty path");

        let trie_account = self.account.into_trie_account(storage_root);
        trie_account.encode(buf);

        Ok(())
    }
}

impl<T, H> LeafValueEncoder for SyncAccountValueEncoder<T, H>
where
    T: TrieCursorFactory,
    H: HashedCursorFactory,
{
    type Value = Account;
    type DeferredEncoder = SyncAccountDeferredValueEncoder<T, H>;

    fn deferred_encoder(
        &mut self,
        hashed_address: B256,
        account: Self::Value,
    ) -> Self::DeferredEncoder {
        // Return a deferred encoder that will synchronously compute the storage root when encode()
        // is called.
        SyncAccountDeferredValueEncoder {
            trie_cursor_factory: self.trie_cursor_factory.clone(),
            hashed_cursor_factory: self.hashed_cursor_factory.clone(),
            hashed_address,
            account,
        }
    }
}
