//! Generic value encoder types for proof calculation with lazy evaluation.

use alloy_primitives::{B256, U256};
use alloy_rlp::Encodable;
use reth_execution_errors::trie::StateProofError;

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
