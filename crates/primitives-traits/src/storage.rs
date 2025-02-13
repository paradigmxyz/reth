use alloy_primitives::{B256, U256};
use rlp::{Decodable, Encodable, Rlp, RlpStream};

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

impl Encodable for StorageEntry {
    fn rlp_append(&self, stream: &mut RlpStream) {
        stream.begin_list(2);
        stream.append(&self.key.to_vec());

        let mut value_bytes = [0u8; 32];
        value_bytes.copy_from_slice(&self.value.to_be_bytes::<32>());
        stream.append(&value_bytes.to_vec());
    }
}

impl Decodable for StorageEntry {
    fn decode(rlp: &Rlp<'_>) -> Result<Self, rlp::DecoderError> {
        let key_bytes: Vec<u8> = rlp.val_at(0)?;
        let value_bytes: Vec<u8> = rlp.val_at(1)?;

        // Decode key
        let key = B256::from_slice(&key_bytes);

        // Convert value bytes back to U256
        let value = U256::from_be_bytes::<32>(
            value_bytes
                .as_slice()
                .try_into()
                .map_err(|_| rlp::DecoderError::Custom("Invalid value length"))?,
        );

        Ok(StorageEntry { key, value })
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
