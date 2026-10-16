//! Generic value encoder types for proof calculation with lazy evaluation.

use super::CachedStorage;
use crate::{hashed_cursor::HashedStorageCursor, trie_cursor::TrieStorageCursor};
use alloy_primitives::{B256, U256};
use alloy_rlp::Encodable;
use parking_lot::Mutex;
use reth_execution_errors::trie::StateProofError;
use reth_primitives_traits::Account;
use std::sync::Arc;

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

pub(super) struct AccountValueEncoder<TC, HC> {
    // Arc<Mutex<_>> is needed only to keep the provider Send;
    // otherwise Rc<RefCell<_>> would suffice.
    pub(super) storage_calculator: Arc<Mutex<CachedStorage<TC, HC>>>,
}

impl<TC: TrieStorageCursor, HC: HashedStorageCursor<Value = U256>> LeafValueEncoder
    for AccountValueEncoder<TC, HC>
{
    type Value = Account;
    type DeferredEncoder = AccountDeferredValueEncoder<TC, HC>;

    fn deferred_encoder(
        &mut self,
        hashed_address: B256,
        account: Account,
    ) -> Self::DeferredEncoder {
        AccountDeferredValueEncoder {
            storage_calculator: self.storage_calculator.clone(),
            hashed_address,
            account,
        }
    }
}

pub(super) struct AccountDeferredValueEncoder<TC, HC> {
    storage_calculator: Arc<Mutex<CachedStorage<TC, HC>>>,
    hashed_address: B256,
    account: Account,
}

impl<TC: TrieStorageCursor, HC: HashedStorageCursor<Value = U256>> DeferredValueEncoder
    for AccountDeferredValueEncoder<TC, HC>
{
    fn encode(self, buf: &mut Vec<u8>) -> Result<(), StateProofError> {
        let root = self
            .storage_calculator
            .lock()
            .calculator(self.hashed_address)
            .storage_root(self.hashed_address)?;
        self.account.into_trie_account(root).encode(buf);
        Ok(())
    }
}
