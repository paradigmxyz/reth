use alloy_primitives::{B256, U256};

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
