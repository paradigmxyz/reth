use alloy_primitives::{keccak256, B256, U256};

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

/// A storage slot key that tracks whether it holds a plain (unhashed) EVM slot
/// or a keccak256-hashed slot.
///
/// This enum replaces the `use_hashed_state: bool` parameter pattern by carrying
/// provenance with the key itself. Once tagged at a read/write boundary, downstream
/// code can call [`Self::to_hashed`] without risk of double-hashing — hashing a
/// [`StorageSlotKey::Hashed`] is a no-op.
///
/// The on-disk encoding is unchanged (raw 32-byte [`B256`]). The variant is set
/// by the code that knows the context (which table, which storage mode).
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StorageSlotKey {
    /// An unhashed EVM storage slot, as produced by REVM execution.
    Plain(B256),
    /// A keccak256-hashed storage slot, as stored in `HashedStorages` and
    /// in v2-mode `StorageChangeSets`.
    Hashed(B256),
}

impl Default for StorageSlotKey {
    fn default() -> Self {
        Self::Plain(B256::ZERO)
    }
}

impl StorageSlotKey {
    /// Create a plain slot key from a REVM [`U256`] storage index.
    pub const fn from_u256(slot: U256) -> Self {
        Self::Plain(B256::new(slot.to_be_bytes()))
    }

    /// Create a plain slot key from a raw [`B256`].
    pub const fn plain(key: B256) -> Self {
        Self::Plain(key)
    }

    /// Create a hashed slot key from a raw [`B256`].
    pub const fn hashed(key: B256) -> Self {
        Self::Hashed(key)
    }

    /// Tag a raw [`B256`] based on the storage mode.
    ///
    /// When `use_hashed_state` is true the key is assumed already hashed.
    /// When false it is assumed to be a plain slot.
    pub const fn from_raw(key: B256, use_hashed_state: bool) -> Self {
        if use_hashed_state {
            Self::Hashed(key)
        } else {
            Self::Plain(key)
        }
    }

    /// Returns the raw [`B256`] regardless of variant.
    pub const fn as_b256(&self) -> B256 {
        match *self {
            Self::Plain(b) | Self::Hashed(b) => b,
        }
    }

    /// Returns `true` if this key is already hashed.
    pub const fn is_hashed(&self) -> bool {
        matches!(self, Self::Hashed(_))
    }

    /// Returns `true` if this key is plain (unhashed).
    pub const fn is_plain(&self) -> bool {
        matches!(self, Self::Plain(_))
    }

    /// Produce the keccak256-hashed form of this slot key.
    ///
    /// - If already [`Hashed`](Self::Hashed), returns the inner value as-is (no double-hash).
    /// - If [`Plain`](Self::Plain), applies keccak256 and returns the result.
    pub fn to_hashed(&self) -> B256 {
        match *self {
            Self::Hashed(b) => b,
            Self::Plain(b) => keccak256(b),
        }
    }

    /// Convert a plain slot to its changeset representation.
    ///
    /// In v2 mode (`use_hashed_state = true`), the changeset stores hashed keys,
    /// so the plain key is hashed. In v1 mode, the plain key is stored as-is.
    ///
    /// Panics (debug) if called on an already-hashed key.
    pub fn to_changeset_key(self, use_hashed_state: bool) -> B256 {
        debug_assert!(self.is_plain(), "to_changeset_key called on already-hashed key");
        if use_hashed_state {
            self.to_hashed()
        } else {
            self.as_b256()
        }
    }

    /// Like [`to_changeset_key`](Self::to_changeset_key) but returns a tagged
    /// [`StorageSlotKey`] instead of a raw [`B256`].
    ///
    /// Panics (debug) if called on an already-hashed key.
    pub fn to_changeset(self, use_hashed_state: bool) -> Self {
        Self::from_raw(self.to_changeset_key(use_hashed_state), use_hashed_state)
    }
}

impl From<StorageSlotKey> for B256 {
    fn from(key: StorageSlotKey) -> Self {
        key.as_b256()
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

    /// Tag this entry's key as a [`StorageSlotKey`] based on the storage mode.
    ///
    /// When `use_hashed_state` is true, the key is tagged as already-hashed.
    /// When false, it is tagged as plain.
    pub const fn slot_key(&self, use_hashed_state: bool) -> StorageSlotKey {
        StorageSlotKey::from_raw(self.key, use_hashed_state)
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
