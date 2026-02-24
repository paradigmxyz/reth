//! RocksDB-backed trie cursor implementations.
//!
//! Provides [`RocksDBTrieCursorFactory`] which implements [`TrieCursorFactory`] using
//! RocksDB column families. Account trie uses `PackedStoredNibbles` (33-byte) keys,
//! while storage trie uses compound keys (`B256 || PackedStoredNibblesSubKey` = 65 bytes)
//! to simulate MDBX's DupSort semantics.

use super::provider::{RocksDBProvider, RocksDBRawIterEnum};
use alloy_primitives::B256;
use reth_db_api::{
    table::{Decode, Decompress, Encode, Table},
    tables, DatabaseError,
};
use reth_trie::{
    trie_cursor::{TrieCursor, TrieCursorFactory, TrieStorageCursor},
    BranchNodeCompact, Nibbles, PackedStoredNibbles, PackedStoredNibblesSubKey,
};

/// RocksDB-backed trie cursor factory.
///
/// Creates cursors that read trie data from RocksDB column families using packed
/// nibble encoding (storage v2). Account trie entries are stored as simple key-value
/// pairs, while storage trie entries use compound keys to flatten MDBX's DupSort layout.
#[derive(Debug)]
pub struct RocksDBTrieCursorFactory<'db> {
    provider: &'db RocksDBProvider,
}

impl<'db> RocksDBTrieCursorFactory<'db> {
    /// Creates a new [`RocksDBTrieCursorFactory`].
    pub const fn new(provider: &'db RocksDBProvider) -> Self {
        Self { provider }
    }
}

impl<'db> TrieCursorFactory for RocksDBTrieCursorFactory<'db> {
    type AccountTrieCursor<'a>
        = RocksDBAccountTrieCursor<'a>
    where
        Self: 'a;

    type StorageTrieCursor<'a>
        = RocksDBStorageTrieCursor<'a>
    where
        Self: 'a;

    fn account_trie_cursor(&self) -> Result<Self::AccountTrieCursor<'_>, DatabaseError> {
        let iter = self.provider.raw_iterator_for_cf(tables::AccountsTrie::NAME)?;
        Ok(RocksDBAccountTrieCursor { iter })
    }

    fn storage_trie_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageTrieCursor<'_>, DatabaseError> {
        // Use bounded iterator scoped to the address prefix.
        // Lower bound: address || 0x00..00  (inclusive)
        // Upper bound: (address + 1) || 0x00..00  (exclusive)
        let mut lower = [0u8; STORAGE_TRIE_ADDRESS_LEN];
        lower.copy_from_slice(hashed_address.as_ref());
        let upper = next_prefix(&lower);
        let iter = self.provider.raw_iterator_for_cf_bounded(
            tables::StoragesTrie::NAME,
            lower.to_vec(),
            upper,
        )?;
        Ok(RocksDBStorageTrieCursor { provider: self.provider, iter, hashed_address })
    }
}

/// RocksDB-backed account trie cursor.
///
/// Iterates over `AccountsTrie` column family entries with `PackedStoredNibbles` keys
/// and `BranchNodeCompact` values.
pub struct RocksDBAccountTrieCursor<'db> {
    iter: RocksDBRawIterEnum<'db>,
}

impl TrieCursor for RocksDBAccountTrieCursor<'_> {
    fn seek_exact(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        let encoded = PackedStoredNibbles::from(key).encode();
        self.iter.seek(&encoded);
        check_iter_status(&self.iter)?;

        if !self.iter.valid() {
            return Ok(None);
        }

        let Some(key_bytes) = self.iter.key() else { return Ok(None) };
        if key_bytes != encoded.as_ref() {
            return Ok(None);
        }

        decode_account_entry(&self.iter)
    }

    fn seek(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        let encoded = PackedStoredNibbles::from(key).encode();
        self.iter.seek(&encoded);
        check_iter_status(&self.iter)?;

        if !self.iter.valid() {
            return Ok(None);
        }

        decode_account_entry(&self.iter)
    }

    fn next(&mut self) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        self.iter.next();
        check_iter_status(&self.iter)?;

        if !self.iter.valid() {
            return Ok(None);
        }

        decode_account_entry(&self.iter)
    }

    fn current(&mut self) -> Result<Option<Nibbles>, DatabaseError> {
        if !self.iter.valid() {
            return Ok(None);
        }

        let Some(key_bytes) = self.iter.key() else { return Ok(None) };
        let key = PackedStoredNibbles::decode(key_bytes)?;
        Ok(Some(key.0))
    }

    fn reset(&mut self) {
        // Seek to the beginning to invalidate current position.
        // Next operation must be a seek.
    }
}

/// RocksDB-backed storage trie cursor.
///
/// Iterates over `StoragesTrie` column family entries using compound keys
/// (`B256 || PackedStoredNibblesSubKey`). Only returns entries matching the
/// current `hashed_address` prefix. Uses bounded iterators to constrain
/// RocksDB to the address prefix range, skipping irrelevant SSTs.
pub struct RocksDBStorageTrieCursor<'db> {
    provider: &'db RocksDBProvider,
    iter: RocksDBRawIterEnum<'db>,
    hashed_address: B256,
}

/// Length of the address prefix in a StoragesTrie compound key.
const STORAGE_TRIE_ADDRESS_LEN: usize = 32;
/// Length of the subkey portion in a StoragesTrie compound key.
const STORAGE_TRIE_SUBKEY_LEN: usize = 33;
/// Total length of a StoragesTrie compound key.
const STORAGE_TRIE_KEY_LEN: usize = STORAGE_TRIE_ADDRESS_LEN + STORAGE_TRIE_SUBKEY_LEN;

impl RocksDBStorageTrieCursor<'_> {
    /// Builds a compound key from the current hashed address and a nibbles subkey.
    fn compound_key(&self, nibbles: PackedStoredNibblesSubKey) -> [u8; STORAGE_TRIE_KEY_LEN] {
        let mut key = [0u8; STORAGE_TRIE_KEY_LEN];
        key[..STORAGE_TRIE_ADDRESS_LEN].copy_from_slice(self.hashed_address.as_ref());
        key[STORAGE_TRIE_ADDRESS_LEN..].copy_from_slice(&nibbles.encode());
        key
    }

    /// Checks if the iterator is positioned at an entry belonging to the current hashed address.
    fn is_current_address(&self) -> bool {
        self.iter.valid() &&
            self.iter.key().is_some_and(|k| {
                k.get(..STORAGE_TRIE_ADDRESS_LEN) == Some(self.hashed_address.as_ref())
            })
    }

    /// Decodes the current iterator entry into `(Nibbles, BranchNodeCompact)`.
    ///
    /// Returns `None` if the iterator is not positioned at the current address.
    fn decode_current(&self) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        if !self.is_current_address() {
            return Ok(None);
        }

        let key_bytes = self.iter.key().ok_or(DatabaseError::Decode)?;
        let subkey_bytes =
            key_bytes.get(STORAGE_TRIE_ADDRESS_LEN..).ok_or(DatabaseError::Decode)?;
        let subkey = PackedStoredNibblesSubKey::decode(subkey_bytes)?;

        let value_bytes = self.iter.value().ok_or(DatabaseError::Decode)?;
        let node = BranchNodeCompact::decompress(value_bytes)?;

        Ok(Some((subkey.0, node)))
    }
}

impl TrieCursor for RocksDBStorageTrieCursor<'_> {
    fn seek_exact(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        let subkey = PackedStoredNibblesSubKey::from(key);
        let encoded_subkey = subkey.clone().encode();
        let compound = self.compound_key(subkey);
        self.iter.seek(&compound);
        check_iter_status(&self.iter)?;

        if !self.is_current_address() {
            return Ok(None);
        }

        // Check for exact match on the subkey portion
        let key_bytes = self.iter.key().ok_or(DatabaseError::Decode)?;
        let current_subkey_bytes =
            key_bytes.get(STORAGE_TRIE_ADDRESS_LEN..).ok_or(DatabaseError::Decode)?;
        if current_subkey_bytes != encoded_subkey.as_ref() {
            return Ok(None);
        }

        self.decode_current()
    }

    fn seek(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        let subkey = PackedStoredNibblesSubKey::from(key);
        let compound = self.compound_key(subkey);
        self.iter.seek(&compound);
        check_iter_status(&self.iter)?;

        self.decode_current()
    }

    fn next(&mut self) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        self.iter.next();
        check_iter_status(&self.iter)?;

        self.decode_current()
    }

    fn current(&mut self) -> Result<Option<Nibbles>, DatabaseError> {
        if !self.is_current_address() {
            return Ok(None);
        }

        let key_bytes = self.iter.key().ok_or(DatabaseError::Decode)?;
        let subkey_bytes =
            key_bytes.get(STORAGE_TRIE_ADDRESS_LEN..).ok_or(DatabaseError::Decode)?;
        let subkey = PackedStoredNibblesSubKey::decode(subkey_bytes)?;
        Ok(Some(subkey.0))
    }

    fn reset(&mut self) {
        // No-op; next operation must be a seek.
    }
}

impl TrieStorageCursor for RocksDBStorageTrieCursor<'_> {
    fn set_hashed_address(&mut self, hashed_address: B256) {
        self.hashed_address = hashed_address;
        // Recreate the bounded iterator for the new address prefix.
        let mut lower = [0u8; STORAGE_TRIE_ADDRESS_LEN];
        lower.copy_from_slice(hashed_address.as_ref());
        let upper = next_prefix(&lower);
        if let Ok(iter) = self.provider.raw_iterator_for_cf_bounded(
            tables::StoragesTrie::NAME,
            lower.to_vec(),
            upper,
        ) {
            self.iter = iter;
        }
    }
}

/// Checks the raw iterator status and converts RocksDB errors to [`DatabaseError`].
fn check_iter_status(iter: &RocksDBRawIterEnum<'_>) -> Result<(), DatabaseError> {
    iter.status().map_err(|e| {
        DatabaseError::Read(reth_storage_errors::db::DatabaseErrorInfo {
            message: e.to_string().into(),
            code: -1,
        })
    })
}

/// Decodes the current account trie entry from the iterator.
fn decode_account_entry(
    iter: &RocksDBRawIterEnum<'_>,
) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
    let key_bytes = iter.key().ok_or(DatabaseError::Decode)?;
    let key = PackedStoredNibbles::decode(key_bytes)?;

    let value_bytes = iter.value().ok_or(DatabaseError::Decode)?;
    let node = BranchNodeCompact::decompress(value_bytes)?;

    Ok(Some((key.0, node)))
}

/// Computes the exclusive upper bound for a prefix by incrementing the last byte.
///
/// For a prefix like `[0x12, 0x34, 0xff]`, increments from the rightmost non-0xff byte
/// to produce `[0x12, 0x35]`. If all bytes are 0xff (e.g., the maximum address),
/// returns a single `[0xff, ..., 0xff, 0xff]` with one extra byte as a safe upper bound.
fn next_prefix(prefix: &[u8]) -> Vec<u8> {
    // Find the rightmost byte that can be incremented
    for i in (0..prefix.len()).rev() {
        if prefix[i] < 0xff {
            let mut upper = prefix[..=i].to_vec();
            upper[i] += 1;
            return upper;
        }
    }
    // All bytes are 0xff — extend with an extra byte to create a bound past the prefix
    let mut upper = prefix.to_vec();
    upper.push(0x00);
    upper
}
