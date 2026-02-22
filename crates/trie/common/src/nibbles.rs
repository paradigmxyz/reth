use alloc::vec::Vec;
use core::cmp::Ordering;
use derive_more::Deref;
pub use nybbles::Nibbles;

/// Compares two [`Nibbles`] in depth-first order.
///
/// In depth-first ordering:
/// - Descendants come before their ancestors (children before parents)
/// - Siblings are ordered lexicographically
pub fn depth_first_cmp(a: &Nibbles, b: &Nibbles) -> Ordering {
    if a.len() == b.len() {
        return a.cmp(b)
    }

    let common_prefix_len = a.common_prefix_length(b);
    if a.len() == common_prefix_len {
        return Ordering::Greater
    } else if b.len() == common_prefix_len {
        return Ordering::Less
    }

    a.get_unchecked(common_prefix_len).cmp(&b.get_unchecked(common_prefix_len))
}

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

#[cfg(any(test, feature = "reth-codec"))]
impl reth_codecs::Compact for StoredNibbles {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let bytes = self.0.iter().collect::<arrayvec::ArrayVec<u8, 64>>();
        buf.put_slice(&bytes);
        bytes.len()
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

impl StoredNibblesSubKey {
    /// Encodes the nibbles into a fixed-size `[u8; 65]` array.
    ///
    /// The first 64 bytes contain the nibble values (right-padded with zeros),
    /// and the 65th byte stores the actual nibble count.
    pub fn to_compact_array(&self) -> [u8; 65] {
        assert!(self.0.len() <= 64);
        let mut buf = [0u8; 65];
        for (i, nibble) in self.0.iter().enumerate() {
            buf[i] = nibble;
        }
        buf[64] = self.0.len() as u8;
        buf
    }
}

#[cfg(any(test, feature = "reth-codec"))]
impl reth_codecs::Compact for StoredNibblesSubKey {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        assert!(self.0.len() <= 64);

        let bytes = self.0.iter().collect::<arrayvec::ArrayVec<u8, 64>>();
        buf.put_slice(&bytes);

        // Right-pad with zeros
        static ZERO: &[u8; 64] = &[0; 64];
        buf.put_slice(&ZERO[bytes.len()..]);

        buf.put_u8(bytes.len() as u8);
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
        let nibbles = Nibbles::from_nibbles_unchecked(vec![0x02, 0x04, 0x06]);
        let stored = StoredNibbles::from(nibbles);
        assert_eq!(stored.0, nibbles);
    }

    #[test]
    fn test_stored_nibbles_from_vec() {
        let bytes = vec![0x02, 0x04, 0x06];
        let stored = StoredNibbles::from(bytes.clone());
        assert_eq!(stored.0.to_vec(), bytes);
    }

    #[test]
    fn test_stored_nibbles_to_compact() {
        let stored = StoredNibbles::from(vec![0x02, 0x04]);
        let mut buf = BytesMut::with_capacity(10);
        let len = stored.to_compact(&mut buf);
        assert_eq!(len, 2);
        assert_eq!(buf, &vec![0x02, 0x04][..]);
    }

    #[test]
    fn test_stored_nibbles_from_compact() {
        let buf = vec![0x02, 0x04, 0x06];
        let (stored, remaining) = StoredNibbles::from_compact(&buf, 2);
        assert_eq!(stored.0.to_vec(), vec![0x02, 0x04]);
        assert_eq!(remaining, &[0x06]);
    }

    #[test]
    fn test_stored_nibbles_subkey_to_compact() {
        let subkey = StoredNibblesSubKey::from(vec![0x02, 0x04]);
        let mut buf = BytesMut::with_capacity(65);
        let len = subkey.to_compact(&mut buf);
        assert_eq!(len, 65);
        assert_eq!(buf[..2], [0x02, 0x04]);
        assert_eq!(buf[64], 2); // Length byte
    }

    #[test]
    fn test_stored_nibbles_subkey_from_compact() {
        let mut buf = vec![0x02, 0x04];
        buf.resize(65, 0);
        buf[64] = 2;
        let (subkey, remaining) = StoredNibblesSubKey::from_compact(&buf, 65);
        assert_eq!(subkey.0.to_vec(), vec![0x02, 0x04]);
        assert_eq!(remaining, &[] as &[u8]);
    }

    #[test]
    fn test_serialization_stored_nibbles() {
        let stored = StoredNibbles::from(vec![0x02, 0x04]);
        let serialized = serde_json::to_string(&stored).unwrap();
        let deserialized: StoredNibbles = serde_json::from_str(&serialized).unwrap();
        assert_eq!(stored, deserialized);
    }

    #[test]
    fn test_serialization_stored_nibbles_subkey() {
        let subkey = StoredNibblesSubKey::from(vec![0x02, 0x04]);
        let serialized = serde_json::to_string(&subkey).unwrap();
        let deserialized: StoredNibblesSubKey = serde_json::from_str(&serialized).unwrap();
        assert_eq!(subkey, deserialized);
    }

    #[test]
    fn test_stored_nibbles_subkey_compact_array_empty() {
        let subkey = StoredNibblesSubKey::from(vec![]);
        let arr = subkey.to_compact_array();
        assert_eq!(arr, [0u8; 65]);
    }

    #[test]
    fn test_stored_nibbles_subkey_compact_array_full() {
        let nibbles: Vec<u8> = (0..64).map(|i| i % 16).collect();
        let subkey = StoredNibblesSubKey::from(nibbles.clone());
        let arr = subkey.to_compact_array();
        assert_eq!(&arr[..64], &nibbles[..]);
        assert_eq!(arr[64], 64);
    }

    #[test]
    fn test_stored_nibbles_subkey_compact_array_roundtrip() {
        let subkey = StoredNibblesSubKey::from(vec![0x0A, 0x0B, 0x0C]);
        let arr = subkey.to_compact_array();
        let (recovered, rest) = StoredNibblesSubKey::from_compact(&arr, 65);
        assert_eq!(recovered, subkey);
        assert!(rest.is_empty());
    }

    #[test]
    fn test_stored_nibbles_subkey_compact_array_matches_to_compact() {
        for len in [0, 1, 2, 31, 32, 33, 63, 64] {
            let nibbles: Vec<u8> = (0..len).map(|i| (i % 16) as u8).collect();
            let subkey = StoredNibblesSubKey::from(nibbles);

            let arr = subkey.to_compact_array();

            let mut compact_buf = BytesMut::with_capacity(65);
            subkey.to_compact(&mut compact_buf);

            assert_eq!(&arr[..], &compact_buf[..], "mismatch at nibble length {len}");
        }
    }

    #[test]
    fn test_stored_nibbles_subkey_compact_array_padding_is_zero() {
        let subkey = StoredNibblesSubKey::from(vec![0x0F; 10]);
        let arr = subkey.to_compact_array();
        assert!(arr[10..64].iter().all(|&b| b == 0), "padding bytes must be zero");
    }
}
