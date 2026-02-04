use alloy_primitives::{B256, U256};
use reth_primitives_traits::ValueWithSubKey;

/// Storage entry as it is saved in the static files.
///
/// [`B256`] is the subkey.
#[derive(Debug, Default, Copy, Clone, Eq, PartialEq)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(any(test, feature = "reth-codec"), reth_codecs::add_arbitrary_tests(compact))]
pub struct StorageBeforeTx {
    /// Hashed address for the storage entry. Acts as `DupSort::SubKey` in static files.
    pub hashed_address: B256,
    /// Storage key (hashed).
    pub key: B256,
    /// Value on storage key.
    pub value: U256,
}

impl ValueWithSubKey for StorageBeforeTx {
    type SubKey = B256;

    fn get_subkey(&self) -> Self::SubKey {
        self.key
    }
}

// NOTE: Removing reth_codec and manually encode subkey
// and compress second part of the value. If we have compression
// over whole value (Even SubKey) that would mess up fetching of values with seek_by_key_subkey
#[cfg(any(test, feature = "reth-codec"))]
impl reth_codecs::Compact for StorageBeforeTx {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        buf.put_slice(self.hashed_address.as_slice());
        buf.put_slice(&self.key[..]);
        self.value.to_compact(buf) + 64
    }

    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        let hashed_address = B256::from_slice(&buf[..32]);
        let key = B256::from_slice(&buf[32..64]);
        let (value, out) = U256::from_compact(&buf[64..], len - 64);
        (Self { hashed_address, key, value }, out)
    }
}
