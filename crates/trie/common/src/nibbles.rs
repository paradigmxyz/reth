use derive_more::Deref;
pub use nybbles::Nibbles;

/// The representation of nibbles of the merkle trie stored in the database.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, derive_more::Index)]
#[cfg_attr(any(test, feature = "serde"), derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "test-utils", derive(arbitrary::Arbitrary))]
pub struct StoredNibbles(pub Nibbles);

impl From<Nibbles> for StoredNibbles {
    #[inline]
    fn from(value: Nibbles) -> Self {
        Self(value)
    }
}

impl From<Vec<u8>> for StoredNibbles {
    #[inline]
    fn from(value: Vec<u8>) -> Self {
        Self(Nibbles::from_nibbles_unchecked(value))
    }
}

impl PartialEq<[u8]> for StoredNibbles {
    #[inline]
    fn eq(&self, other: &[u8]) -> bool {
        self.0.as_slice() == other
    }
}

impl PartialOrd<[u8]> for StoredNibbles {
    #[inline]
    fn partial_cmp(&self, other: &[u8]) -> Option<std::cmp::Ordering> {
        self.0.as_slice().partial_cmp(other)
    }
}

impl core::borrow::Borrow<[u8]> for StoredNibbles {
    #[inline]
    fn borrow(&self) -> &[u8] {
        self.0.as_slice()
    }
}

#[cfg(any(test, feature = "reth-codec"))]
impl reth_codecs::Compact for StoredNibbles {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        buf.put_slice(self.0.as_slice());
        self.0.len()
    }

    fn from_compact(mut buf: &[u8], len: usize) -> (Self, &[u8]) {
        use bytes::Buf;

        let nibbles = &buf[..len];
        buf.advance(len);
        (Self(Nibbles::from_nibbles_unchecked(nibbles)), buf)
    }
}

/// The representation of nibbles of the merkle trie stored in the database.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Deref)]
#[cfg_attr(any(test, feature = "serde"), derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "test-utils", derive(arbitrary::Arbitrary))]
pub struct StoredNibblesSubKey(pub Nibbles);

impl From<Nibbles> for StoredNibblesSubKey {
    #[inline]
    fn from(value: Nibbles) -> Self {
        Self(value)
    }
}

impl From<Vec<u8>> for StoredNibblesSubKey {
    #[inline]
    fn from(value: Vec<u8>) -> Self {
        Self(Nibbles::from_nibbles_unchecked(value))
    }
}

impl From<StoredNibblesSubKey> for Nibbles {
    #[inline]
    fn from(value: StoredNibblesSubKey) -> Self {
        value.0
    }
}

#[cfg(any(test, feature = "reth-codec"))]
impl reth_codecs::Compact for StoredNibblesSubKey {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        assert!(self.0.len() <= 64);

        // right-pad with zeros
        buf.put_slice(&self.0[..]);
        static ZERO: &[u8; 64] = &[0; 64];
        buf.put_slice(&ZERO[self.0.len()..]);

        buf.put_u8(self.0.len() as u8);
        64 + 1
    }

    fn from_compact(buf: &[u8], _len: usize) -> (Self, &[u8]) {
        let len = buf[64] as usize;
        (Self(Nibbles::from_nibbles_unchecked(&buf[..len])), &buf[65..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::BytesMut;
    use reth_codecs::Compact;

    #[test]
    fn test_stored_nibbles_from_nibbles() {
        let nibbles = Nibbles::from_nibbles_unchecked(vec![0x12, 0x34, 0x56]);
        let stored = StoredNibbles::from(nibbles.clone());
        assert_eq!(stored.0, nibbles);
    }

    #[test]
    fn test_stored_nibbles_from_vec() {
        let bytes = vec![0x12, 0x34, 0x56];
        let stored = StoredNibbles::from(bytes.clone());
        assert_eq!(stored.0.as_slice(), bytes.as_slice());
    }

    #[test]
    fn test_stored_nibbles_equality() {
        let bytes = vec![0x12, 0x34];
        let stored = StoredNibbles::from(bytes.clone());
        assert_eq!(stored, *bytes.as_slice());
    }

    #[test]
    fn test_stored_nibbles_partial_cmp() {
        let stored = StoredNibbles::from(vec![0x12, 0x34]);
        let other = vec![0x12, 0x35];
        assert!(stored < *other.as_slice());
    }

    #[test]
    fn test_stored_nibbles_to_compact() {
        let stored = StoredNibbles::from(vec![0x12, 0x34]);
        let mut buf = BytesMut::with_capacity(10);
        let len = stored.to_compact(&mut buf);
        assert_eq!(len, 2);
        assert_eq!(buf, &vec![0x12, 0x34][..]);
    }

    #[test]
    fn test_stored_nibbles_from_compact() {
        let buf = vec![0x12, 0x34, 0x56];
        let (stored, remaining) = StoredNibbles::from_compact(&buf, 2);
        assert_eq!(stored.0.as_slice(), &[0x12, 0x34]);
        assert_eq!(remaining, &[0x56]);
    }

    #[test]
    fn test_stored_nibbles_subkey_from_nibbles() {
        let nibbles = Nibbles::from_nibbles_unchecked(vec![0x12, 0x34]);
        let subkey = StoredNibblesSubKey::from(nibbles.clone());
        assert_eq!(subkey.0, nibbles);
    }

    #[test]
    fn test_stored_nibbles_subkey_to_compact() {
        let subkey = StoredNibblesSubKey::from(vec![0x12, 0x34]);
        let mut buf = BytesMut::with_capacity(65);
        let len = subkey.to_compact(&mut buf);
        assert_eq!(len, 65);
        assert_eq!(buf[..2], [0x12, 0x34]);
        assert_eq!(buf[64], 2); // Length byte
    }

    #[test]
    fn test_stored_nibbles_subkey_from_compact() {
        let mut buf = vec![0x12, 0x34];
        buf.resize(65, 0);
        buf[64] = 2;
        let (subkey, remaining) = StoredNibblesSubKey::from_compact(&buf, 65);
        assert_eq!(subkey.0.as_slice(), &[0x12, 0x34]);
        assert_eq!(remaining, &[] as &[u8]);
    }

    #[test]
    fn test_serialization_stored_nibbles() {
        let stored = StoredNibbles::from(vec![0x12, 0x34]);
        let serialized = serde_json::to_string(&stored).unwrap();
        let deserialized: StoredNibbles = serde_json::from_str(&serialized).unwrap();
        assert_eq!(stored, deserialized);
    }

    #[test]
    fn test_serialization_stored_nibbles_subkey() {
        let subkey = StoredNibblesSubKey::from(vec![0x12, 0x34]);
        let serialized = serde_json::to_string(&subkey).unwrap();
        let deserialized: StoredNibblesSubKey = serde_json::from_str(&serialized).unwrap();
        assert_eq!(subkey, deserialized);
    }
}
