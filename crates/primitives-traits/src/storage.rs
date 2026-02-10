use alloy_primitives::{keccak256, B256, U256};
use core::marker::PhantomData;

/// Trait for `DupSort` table values that contain a subkey.
///
/// This trait allows extracting the subkey from a value during database iteration,
/// enabling proper range queries and filtering on `DupSort` tables.
pub trait ValueWithSubKey {
    /// The type of the subkey.
    type SubKey;

    /// Extract the subkey from the value.
    fn get_subkey(&self) -> Self::SubKey;
}

/// Marker type for an unhashed (plain) EVM storage slot.
#[derive(Debug)]
pub enum PlainSlot {}

/// Marker type for a keccak256-hashed storage slot.
#[derive(Debug)]
pub enum HashedSlot {}

/// A storage slot key tagged with its hashing status.
///
/// This is a zero-cost wrapper around [`B256`] that uses a marker type parameter
/// to distinguish plain slots from hashed slots at the type level. The on-disk
/// encoding is identical to a raw [`B256`] — the marker exists only in the Rust
/// type system to prevent double-hashing bugs at compile time.
#[repr(transparent)]
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Default)]
pub struct StorageSlotKey<K> {
    inner: B256,
    _marker: PhantomData<K>,
}

/// A plain (unhashed) storage slot key.
pub type PlainSlotKey = StorageSlotKey<PlainSlot>;

/// A keccak256-hashed storage slot key.
pub type HashedSlotKey = StorageSlotKey<HashedSlot>;

impl<K> StorageSlotKey<K> {
    /// Wrap a raw [`B256`] as a typed slot key.
    ///
    /// The caller is responsible for ensuring that the value matches the marker
    /// type `K` (i.e. that a plain B256 is wrapped as [`PlainSlotKey`] and a
    /// hashed B256 is wrapped as [`HashedSlotKey`]).
    pub const fn new_unchecked(inner: B256) -> Self {
        Self { inner, _marker: PhantomData }
    }

    /// Returns a reference to the inner [`B256`].
    pub const fn as_b256(&self) -> &B256 {
        &self.inner
    }

    /// Consumes self and returns the inner [`B256`].
    pub const fn into_b256(self) -> B256 {
        self.inner
    }
}

impl PlainSlotKey {
    /// Create a [`PlainSlotKey`] from a REVM [`U256`] storage index.
    pub const fn from_u256(slot: U256) -> Self {
        Self::new_unchecked(B256::new(slot.to_be_bytes()))
    }

    /// Hash this plain slot key with keccak256, producing a [`HashedSlotKey`].
    pub fn hash(self) -> HashedSlotKey {
        HashedSlotKey::new_unchecked(keccak256(self.inner))
    }

    /// Conditionally hash based on mode: returns [`HashedSlotKey`] (hashed) if
    /// `use_hashed_state` is true, otherwise wraps the plain key as-is.
    ///
    /// This is the typed replacement for the `if use_hashed { keccak256(key) } else { key }`
    /// pattern at REVM→changeset boundaries.
    pub fn to_changeset_key(self, use_hashed_state: bool) -> B256 {
        if use_hashed_state { self.hash().into_b256() } else { self.into_b256() }
    }
}

impl HashedSlotKey {
    /// Use a changeset key that is already hashed as-is for hashed-state lookups.
    ///
    /// This is the typed replacement for the `if use_hashed { key } else { keccak256(key) }`
    /// pattern at changeset→trie boundaries in v2 mode.
    pub const fn as_hashed(self) -> B256 {
        self.inner
    }
}

impl<K> From<StorageSlotKey<K>> for B256 {
    fn from(key: StorageSlotKey<K>) -> Self {
        key.inner
    }
}

/// A typed in-memory view of a [`StorageEntry`] whose slot key is tagged as
/// either [`PlainSlot`] or [`HashedSlot`].
///
/// The raw [`StorageEntry`] remains the on-disk / codec type used by DB tables.
/// This wrapper is used at provider boundaries to carry provenance information
/// through higher-level code without changing the storage format.
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Debug, Default)]
pub struct TypedStorageEntry<K> {
    /// Typed storage slot key.
    pub key: StorageSlotKey<K>,
    /// Value at this storage slot.
    pub value: U256,
}

/// A [`TypedStorageEntry`] with a plain (unhashed) slot key.
pub type PlainStorageEntry = TypedStorageEntry<PlainSlot>;

/// A [`TypedStorageEntry`] with a keccak256-hashed slot key.
pub type HashedStorageEntry = TypedStorageEntry<HashedSlot>;

impl<K> From<TypedStorageEntry<K>> for StorageEntry {
    fn from(e: TypedStorageEntry<K>) -> Self {
        Self { key: e.key.into_b256(), value: e.value }
    }
}

impl<K> From<StorageEntry> for TypedStorageEntry<K> {
    fn from(e: StorageEntry) -> Self {
        Self { key: StorageSlotKey::new_unchecked(e.key), value: e.value }
    }
}

impl<K> ValueWithSubKey for TypedStorageEntry<K> {
    type SubKey = B256;

    fn get_subkey(&self) -> Self::SubKey {
        *self.key.as_b256()
    }
}

/// Account storage entry.
///
/// `key` is the subkey when used as a value in the `StorageChangeSets` table.
#[derive(Debug, Default, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[cfg_attr(any(test, feature = "reth-codec"), reth_codecs::add_arbitrary_tests(compact))]
pub struct StorageEntry {
    /// Storage key.
    pub key: B256,
    /// Value on storage key.
    pub value: U256,
}

impl StorageEntry {
    /// Create a new `StorageEntry` with given key and value.
    pub const fn new(key: B256, value: U256) -> Self {
        Self { key, value }
    }

    /// Interpret this entry as a [`TypedStorageEntry`] with the given marker.
    ///
    /// The caller must ensure the marker matches reality.
    pub const fn into_typed<K>(self) -> TypedStorageEntry<K> {
        TypedStorageEntry { key: StorageSlotKey::new_unchecked(self.key), value: self.value }
    }

    /// Convert the changeset key to a hashed slot key for trie lookups.
    ///
    /// When `use_hashed_state` is true the key is already hashed (passthrough).
    /// When false the key is plain and needs keccak256 hashing.
    pub fn changeset_key_to_hashed_slot(&self, use_hashed_state: bool) -> B256 {
        if use_hashed_state { self.key } else { keccak256(self.key) }
    }
}

impl ValueWithSubKey for StorageEntry {
    type SubKey = B256;

    fn get_subkey(&self) -> Self::SubKey {
        self.key
    }
}

impl From<(B256, U256)> for StorageEntry {
    fn from((key, value): (B256, U256)) -> Self {
        Self { key, value }
    }
}

// NOTE: Removing reth_codec and manually encode subkey
// and compress second part of the value. If we have compression
// over whole value (Even SubKey) that would mess up fetching of values with seek_by_key_subkey
#[cfg(any(test, feature = "reth-codec"))]
impl reth_codecs::Compact for StorageEntry {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        // for now put full bytes and later compress it.
        buf.put_slice(&self.key[..]);
        self.value.to_compact(buf) + 32
    }

    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        let key = B256::from_slice(&buf[..32]);
        let (value, out) = U256::from_compact(&buf[32..], len - 32);
        (Self { key, value }, out)
    }
}
