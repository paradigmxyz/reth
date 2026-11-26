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

#[cfg(any(test, feature = "reth-codec"))]
reth_codecs::impl_compression_for_compact!(StoredNibbles);

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

#[cfg(any(test, feature = "reth-codec"))]
reth_codecs::impl_compression_for_compact!(StoredNibblesSubKey);

/// Packed representation of nibbles for the `AccountsTrie` (storage v2).
///
/// Stores 2 nibbles per byte via [`Nibbles::pack`], right-padded to 32 bytes + 1 nibble-count
/// byte = 33 bytes fixed. This halves the key size compared to [`StoredNibbles`] while
/// preserving sort order under `memcmp`.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, derive_more::Index)]
#[cfg_attr(any(test, feature = "serde"), derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "test-utils", derive(arbitrary::Arbitrary))]
pub struct PackedStoredNibbles(pub Nibbles);

impl From<Nibbles> for PackedStoredNibbles {
    #[inline]
    fn from(value: Nibbles) -> Self {
        Self(value)
    }
}

impl From<Vec<u8>> for PackedStoredNibbles {
    #[inline]
    fn from(value: Vec<u8>) -> Self {
        Self(Nibbles::from_nibbles_unchecked(value))
    }
}

impl From<StoredNibbles> for PackedStoredNibbles {
    #[inline]
    fn from(value: StoredNibbles) -> Self {
        Self(value.0)
    }
}

impl From<PackedStoredNibbles> for StoredNibbles {
    #[inline]
    fn from(value: PackedStoredNibbles) -> Self {
        Self(value.0)
    }
}

impl PackedStoredNibbles {
    /// Encodes the nibbles into a fixed-size `[u8; 33]` array.
    ///
    /// The first 32 bytes contain the packed nibbles (2 per byte, right-padded with zeros),
    /// and the 33rd byte stores the actual nibble count.
    pub fn to_compact_array(&self) -> [u8; 33] {
        assert!(self.0.len() <= 64);
        let mut buf = [0u8; 33];
        self.0.pack_to(&mut buf[..32]);
        buf[32] = self.0.len() as u8;
        buf
    }
}

#[cfg(any(test, feature = "reth-codec"))]
impl reth_codecs::Compact for PackedStoredNibbles {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        assert!(self.0.len() <= 64);

        let mut packed = [0u8; 32];
        self.0.pack_to(&mut packed);
        buf.put_slice(&packed);
        buf.put_u8(self.0.len() as u8);
        33
    }

    fn from_compact(buf: &[u8], _len: usize) -> (Self, &[u8]) {
        let nibble_count = buf[32] as usize;
        let packed_len = nibble_count.div_ceil(2);
        (Self(Nibbles::unpack(&buf[..packed_len]).slice(..nibble_count)), &buf[33..])
    }
}

#[cfg(any(test, feature = "reth-codec"))]
reth_codecs::impl_compression_for_compact!(PackedStoredNibbles);

/// Packed representation of nibbles as a `DupSort` subkey for `StoragesTrie` (storage v2).
///
/// Stores 2 nibbles per byte via [`Nibbles::pack`], right-padded to 32 bytes + 1 nibble-count
/// byte = 33 bytes fixed. This halves the subkey size compared to [`StoredNibblesSubKey`]
/// (65 → 33 bytes) while preserving sort order under `memcmp`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Deref)]
#[cfg_attr(any(test, feature = "serde"), derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "test-utils", derive(arbitrary::Arbitrary))]
pub struct PackedStoredNibblesSubKey(pub Nibbles);

impl From<Nibbles> for PackedStoredNibblesSubKey {
    #[inline]
    fn from(value: Nibbles) -> Self {
        Self(value)
    }
}

impl From<Vec<u8>> for PackedStoredNibblesSubKey {
    #[inline]
    fn from(value: Vec<u8>) -> Self {
        Self(Nibbles::from_nibbles_unchecked(value))
    }
}

impl From<PackedStoredNibblesSubKey> for Nibbles {
    #[inline]
    fn from(value: PackedStoredNibblesSubKey) -> Self {
        value.0
    }
}

impl From<StoredNibblesSubKey> for PackedStoredNibblesSubKey {
    #[inline]
    fn from(value: StoredNibblesSubKey) -> Self {
        Self(value.0)
    }
}

impl From<PackedStoredNibblesSubKey> for StoredNibblesSubKey {
    #[inline]
    fn from(value: PackedStoredNibblesSubKey) -> Self {
        Self(value.0)
    }
}

impl PackedStoredNibblesSubKey {
    /// Encodes the nibbles into a fixed-size `[u8; 33]` array.
    ///
    /// The first 32 bytes contain the packed nibbles (2 per byte, right-padded with zeros),
    /// and the 33rd byte stores the actual nibble count.
    pub fn to_compact_array(&self) -> [u8; 33] {
        assert!(self.0.len() <= 64);
        let mut buf = [0u8; 33];
        self.0.pack_to(&mut buf[..32]);
        buf[32] = self.0.len() as u8;
        buf
    }
}

#[cfg(any(test, feature = "reth-codec"))]
impl reth_codecs::Compact for PackedStoredNibblesSubKey {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        assert!(self.0.len() <= 64);

        let mut packed = [0u8; 32];
        self.0.pack_to(&mut packed);
        buf.put_slice(&packed);
        buf.put_u8(self.0.len() as u8);
        33
    }

    fn from_compact(buf: &[u8], _len: usize) -> (Self, &[u8]) {
        let nibble_count = buf[32] as usize;
        let packed_len = nibble_count.div_ceil(2);
        (Self(Nibbles::unpack(&buf[..packed_len]).slice(..nibble_count)), &buf[33..])
    }
}

#[cfg(any(test, feature = "reth-codec"))]
reth_codecs::impl_compression_for_compact!(PackedStoredNibblesSubKey);

/// Maximum length in bytes of a memcmp group encoding, reached by a 64-nibble path (8 groups of
/// 4 bytes + 1 marker byte each).
///
/// See [`MemcmpStoredNibbles`] for the layout.
pub const MEMCMP_NIBBLES_MAX_ENCODED_LEN: usize = 40;

/// Number of nibbles stored per group of the memcmp group encoding.
const MEMCMP_GROUP_NIBBLES: usize = 8;

/// Number of packed bytes per group of the memcmp group encoding.
const MEMCMP_GROUP_BYTES: usize = MEMCMP_GROUP_NIBBLES / 2;

/// Size of a group plus its trailing marker byte.
const MEMCMP_GROUP_ENCODED_LEN: usize = MEMCMP_GROUP_BYTES + 1;

/// Marker byte meaning more groups follow the current one.
///
/// It is greater than every final-group marker (`0..=8`), so a path sorts before its extensions.
const MEMCMP_MARKER_MORE: u8 = 9;

/// Memcmp-ordered, variable-length representation of nibbles for the `AccountsTrie`.
///
/// Uses the memcmp group encoding (as in `MyRocks` / `TiKV` memcomparable keys):
/// - The path is split into groups of 8 nibbles. Each group is written as 4 bytes, two nibbles per
///   byte, high nibble first. The last group is right-padded with zero nibbles.
/// - Each group is followed by a marker byte: `9` if more groups follow, otherwise the number of
///   nibbles used in this group (`0..=8`).
///
/// Examples: the empty path is `00 00 00 00 00`, 8 nibbles take one group with marker `08`, and
/// 9 nibbles take `group1 09 group2 01`. Encodings are `5 * max(1, ceil(len / 8))` bytes, at most
/// [`MEMCMP_NIBBLES_MAX_ENCODED_LEN`] bytes for 64 nibbles. Mainnet trie paths are mostly at most
/// 8 nibbles, so most keys take 5 bytes instead of 33 for [`PackedStoredNibbles`].
///
/// The encoding is:
/// - order-preserving: `memcmp` order of encodings equals the [`Ord`] of [`Nibbles`], with a path
///   sorting before its extensions;
/// - injective and prefix-free: no encoding is a proper prefix of another;
/// - self-delimiting: the decoder finds the end from the markers, without an external length.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, derive_more::Index)]
#[cfg_attr(any(test, feature = "serde"), derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "test-utils", derive(arbitrary::Arbitrary))]
pub struct MemcmpStoredNibbles(pub Nibbles);

impl From<Nibbles> for MemcmpStoredNibbles {
    #[inline]
    fn from(value: Nibbles) -> Self {
        Self(value)
    }
}

impl MemcmpStoredNibbles {
    /// Encodes the nibbles with the memcmp group encoding into a stack buffer.
    ///
    /// Produces the same bytes as [`Compact::to_compact`](reth_codecs::Compact::to_compact).
    ///
    /// # Panics
    ///
    /// Panics if the path is longer than 64 nibbles.
    #[cfg(any(test, feature = "reth-codec"))]
    #[inline]
    pub fn to_compact_array(&self) -> arrayvec::ArrayVec<u8, MEMCMP_NIBBLES_MAX_ENCODED_LEN> {
        encode_memcmp_nibbles(&self.0)
    }
}

/// Writes the memcmp group encoding and returns its length.
///
/// `from_compact` ignores `len` and finds the end of the encoding from the markers, returning the
/// bytes after it. It panics on input that [`try_decode_memcmp_nibbles`] rejects.
#[cfg(any(test, feature = "reth-codec"))]
impl reth_codecs::Compact for MemcmpStoredNibbles {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let encoded = encode_memcmp_nibbles(&self.0);
        buf.put_slice(&encoded);
        encoded.len()
    }

    fn from_compact(buf: &[u8], _len: usize) -> (Self, &[u8]) {
        let Some((nibbles, rest)) = try_decode_memcmp_nibbles(buf) else {
            panic!("malformed memcmp nibbles encoding")
        };
        (Self(nibbles), rest)
    }
}

#[cfg(any(test, feature = "reth-codec"))]
reth_codecs::impl_compression_for_compact!(MemcmpStoredNibbles);

/// Memcmp-ordered, variable-length representation of nibbles as a `DupSort` subkey for
/// `StoragesTrie`.
///
/// Uses the same memcmp group encoding as [`MemcmpStoredNibbles`]. Because the encoding is
/// prefix-free and order-preserving, `subkey || node` values sort by subkey under `memcmp`, and a
/// seek with the bare subkey lands on the entry with that subkey, if any.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Deref)]
#[cfg_attr(any(test, feature = "serde"), derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "test-utils", derive(arbitrary::Arbitrary))]
pub struct MemcmpStoredNibblesSubKey(pub Nibbles);

impl From<Nibbles> for MemcmpStoredNibblesSubKey {
    #[inline]
    fn from(value: Nibbles) -> Self {
        Self(value)
    }
}

impl MemcmpStoredNibblesSubKey {
    /// Encodes the nibbles with the memcmp group encoding into a stack buffer.
    ///
    /// Produces the same bytes as [`Compact::to_compact`](reth_codecs::Compact::to_compact).
    ///
    /// # Panics
    ///
    /// Panics if the path is longer than 64 nibbles.
    #[cfg(any(test, feature = "reth-codec"))]
    #[inline]
    pub fn to_compact_array(&self) -> arrayvec::ArrayVec<u8, MEMCMP_NIBBLES_MAX_ENCODED_LEN> {
        encode_memcmp_nibbles(&self.0)
    }
}

/// Writes the memcmp group encoding and returns its length.
///
/// `from_compact` ignores `len` and finds the end of the encoding from the markers, returning the
/// bytes after it, so it can be followed by the node in a `StoragesTrie` value. It panics on
/// input that [`try_decode_memcmp_nibbles`] rejects.
#[cfg(any(test, feature = "reth-codec"))]
impl reth_codecs::Compact for MemcmpStoredNibblesSubKey {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let encoded = encode_memcmp_nibbles(&self.0);
        buf.put_slice(&encoded);
        encoded.len()
    }

    fn from_compact(buf: &[u8], _len: usize) -> (Self, &[u8]) {
        let Some((nibbles, rest)) = try_decode_memcmp_nibbles(buf) else {
            panic!("malformed memcmp nibbles encoding")
        };
        (Self(nibbles), rest)
    }
}

#[cfg(any(test, feature = "reth-codec"))]
reth_codecs::impl_compression_for_compact!(MemcmpStoredNibblesSubKey);

/// Returns the length of the memcmp group encoding of a path of `nibble_len` nibbles.
///
/// The empty path still takes one (all-zero) group.
#[cfg(any(test, feature = "reth-codec"))]
#[inline]
const fn memcmp_encoded_len(nibble_len: usize) -> usize {
    let groups = nibble_len.div_ceil(MEMCMP_GROUP_NIBBLES);
    let groups = if groups == 0 { 1 } else { groups };
    groups * MEMCMP_GROUP_ENCODED_LEN
}

/// Encodes `nibbles` with the memcmp group encoding, without heap allocation.
///
/// See [`MemcmpStoredNibbles`] for the layout.
#[cfg(any(test, feature = "reth-codec"))]
#[inline]
fn encode_memcmp_nibbles(
    nibbles: &Nibbles,
) -> arrayvec::ArrayVec<u8, MEMCMP_NIBBLES_MAX_ENCODED_LEN> {
    let nibble_len = nibbles.len();
    assert!(nibble_len <= 64);

    // `pack_to` zero-fills the low half of the last byte for odd lengths, and the zeroed buffer
    // supplies the remaining padding.
    let mut packed = [0u8; 32];
    nibbles.pack_to(&mut packed);

    let encoded_len = memcmp_encoded_len(nibble_len);
    let groups = encoded_len / MEMCMP_GROUP_ENCODED_LEN;
    let mut out = [0u8; MEMCMP_NIBBLES_MAX_ENCODED_LEN];
    let (out_groups, _) = out.as_chunks_mut::<MEMCMP_GROUP_ENCODED_LEN>();
    let (packed_groups, _) = packed.as_chunks::<MEMCMP_GROUP_BYTES>();
    for (group, (dst, src)) in out_groups.iter_mut().zip(packed_groups).take(groups).enumerate() {
        dst[..MEMCMP_GROUP_BYTES].copy_from_slice(src);
        dst[MEMCMP_GROUP_BYTES] = if group + 1 < groups {
            MEMCMP_MARKER_MORE
        } else {
            // The last group holds `1..=8` nibbles, or `0` for the empty path.
            (nibble_len - group * MEMCMP_GROUP_NIBBLES) as u8
        };
    }

    let mut encoded = arrayvec::ArrayVec::from(out);
    encoded.truncate(encoded_len);
    encoded
}

/// Decodes a memcmp group encoding from the start of `buf`, returning the nibbles and the bytes
/// after the encoding.
///
/// Accepts exactly the encodings that [`MemcmpStoredNibbles`] produces, see its docs for the
/// layout. Returns `None` if `buf` ends before the final marker, if the encoding has more than 8
/// groups, if a non-final marker is not `9`, if the final marker is greater than `8`, or `0` after
/// another group, or if any bit after the last nibble of the final group is set.
pub fn try_decode_memcmp_nibbles(buf: &[u8]) -> Option<(Nibbles, &[u8])> {
    let mut packed = [0u8; 32];
    let (groups, _) = buf.as_chunks::<MEMCMP_GROUP_ENCODED_LEN>();
    let max_groups = MEMCMP_NIBBLES_MAX_ENCODED_LEN / MEMCMP_GROUP_ENCODED_LEN;
    for (group, &[bytes @ .., marker]) in groups.iter().take(max_groups).enumerate() {
        packed[group * MEMCMP_GROUP_BYTES..][..MEMCMP_GROUP_BYTES].copy_from_slice(&bytes);
        if marker == MEMCMP_MARKER_MORE {
            continue
        }

        // Bits after the last used nibble. The shift overflows to an empty mask for a full group.
        let padding_mask = u32::MAX.checked_shr(4 * u32::from(marker)).unwrap_or(0);
        if usize::from(marker) > MEMCMP_GROUP_NIBBLES ||
            (marker == 0 && group > 0) ||
            u32::from_be_bytes(bytes) & padding_mask != 0
        {
            return None
        }

        let nibble_len = group * MEMCMP_GROUP_NIBBLES + usize::from(marker);
        let nibbles = Nibbles::unpack(&packed[..nibble_len.div_ceil(2)]).slice(..nibble_len);
        return Some((nibbles, &buf[(group + 1) * MEMCMP_GROUP_ENCODED_LEN..]))
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::hex;
    use bytes::BytesMut;
    use proptest::{collection::vec, prelude::*};
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
    fn test_packed_stored_nibbles_roundtrip() {
        let stored = PackedStoredNibbles::from(vec![0x0A, 0x0B, 0x0C]);
        let mut buf = BytesMut::with_capacity(33);
        let len = stored.to_compact(&mut buf);
        assert_eq!(len, 33);
        assert_eq!(buf[0], 0xAB);
        assert_eq!(buf[1], 0xC0);
        assert_eq!(buf[32], 3);

        let (roundtrip, _) = PackedStoredNibbles::from_compact(&buf, 33);
        assert_eq!(roundtrip.0.to_vec(), vec![0x0A, 0x0B, 0x0C]);
    }

    #[test]
    fn test_packed_stored_nibbles_subkey_roundtrip() {
        let subkey = PackedStoredNibblesSubKey::from(vec![0x02, 0x04]);
        let mut buf = BytesMut::with_capacity(33);
        let len = subkey.to_compact(&mut buf);
        assert_eq!(len, 33);
        assert_eq!(buf[0], 0x24);
        assert_eq!(buf[32], 2);

        let (roundtrip, _) = PackedStoredNibblesSubKey::from_compact(&buf, 33);
        assert_eq!(roundtrip.0.to_vec(), vec![0x02, 0x04]);
    }

    #[test]
    fn test_packed_sort_order_preserved() {
        let cases: Vec<Vec<u8>> = vec![
            vec![0x01],
            vec![0x01, 0x00],
            vec![0x01, 0x02],
            vec![0x02],
            vec![0x03, 0x0A],
            vec![0x03, 0x0B],
            vec![0x04, 0x00],
            vec![0x0A, 0x0B, 0x0C, 0x0D],
        ];

        let mut packed_bufs: Vec<Vec<u8>> = Vec::new();
        for nibbles in &cases {
            let subkey = PackedStoredNibblesSubKey::from(nibbles.clone());
            let mut buf = BytesMut::with_capacity(33);
            subkey.to_compact(&mut buf);
            packed_bufs.push(buf.to_vec());
        }

        for i in 1..packed_bufs.len() {
            assert!(
                packed_bufs[i - 1] < packed_bufs[i],
                "sort order broken: {:?} should be < {:?}",
                cases[i - 1],
                cases[i]
            );
        }
    }

    #[test]
    fn test_packed_legacy_conversion() {
        let nibbles = vec![0x0A, 0x0B, 0x0C, 0x0D];
        let legacy = StoredNibblesSubKey::from(nibbles);
        let packed: PackedStoredNibblesSubKey = legacy.clone().into();
        let back: StoredNibblesSubKey = packed.into();
        assert_eq!(legacy, back);
    }

    #[test]
    fn test_packed_stored_nibbles_compact_array_empty() {
        let stored = PackedStoredNibbles::from(vec![]);
        let arr = stored.to_compact_array();
        assert_eq!(arr, [0u8; 33]);
    }

    #[test]
    fn test_packed_stored_nibbles_compact_array_full() {
        let nibbles: Vec<u8> = (0..64).map(|i| i % 16).collect();
        let stored = PackedStoredNibbles::from(nibbles);
        let arr = stored.to_compact_array();
        assert_eq!(arr[32], 64);
        let (recovered, rest) = PackedStoredNibbles::from_compact(&arr, 33);
        assert_eq!(recovered, stored);
        assert!(rest.is_empty());
    }

    #[test]
    fn test_packed_stored_nibbles_compact_array_roundtrip() {
        let stored = PackedStoredNibbles::from(vec![0x0A, 0x0B, 0x0C]);
        let arr = stored.to_compact_array();
        let (recovered, rest) = PackedStoredNibbles::from_compact(&arr, 33);
        assert_eq!(recovered, stored);
        assert!(rest.is_empty());
    }

    #[test]
    fn test_packed_stored_nibbles_compact_array_matches_to_compact() {
        for len in [0, 1, 2, 31, 32, 33, 63, 64] {
            let nibbles: Vec<u8> = (0..len).map(|i| (i % 16) as u8).collect();
            let stored = PackedStoredNibbles::from(nibbles);
            let arr = stored.to_compact_array();
            let mut compact_buf = BytesMut::with_capacity(33);
            stored.to_compact(&mut compact_buf);
            assert_eq!(&arr[..], &compact_buf[..], "mismatch at nibble length {len}");
        }
    }

    #[test]
    fn test_packed_stored_nibbles_subkey_compact_array_empty() {
        let subkey = PackedStoredNibblesSubKey::from(vec![]);
        let arr = subkey.to_compact_array();
        assert_eq!(arr, [0u8; 33]);
    }

    #[test]
    fn test_packed_stored_nibbles_subkey_compact_array_full() {
        let nibbles: Vec<u8> = (0..64).map(|i| i % 16).collect();
        let subkey = PackedStoredNibblesSubKey::from(nibbles);
        let arr = subkey.to_compact_array();
        assert_eq!(arr[32], 64);
        let (recovered, rest) = PackedStoredNibblesSubKey::from_compact(&arr, 33);
        assert_eq!(recovered, subkey);
        assert!(rest.is_empty());
    }

    #[test]
    fn test_packed_stored_nibbles_subkey_compact_array_roundtrip() {
        let subkey = PackedStoredNibblesSubKey::from(vec![0x0A, 0x0B, 0x0C]);
        let arr = subkey.to_compact_array();
        let (recovered, rest) = PackedStoredNibblesSubKey::from_compact(&arr, 33);
        assert_eq!(recovered, subkey);
        assert!(rest.is_empty());
    }

    #[test]
    fn test_packed_stored_nibbles_subkey_compact_array_matches_to_compact() {
        for len in [0, 1, 2, 31, 32, 33, 63, 64] {
            let nibbles: Vec<u8> = (0..len).map(|i| (i % 16) as u8).collect();
            let subkey = PackedStoredNibblesSubKey::from(nibbles);
            let arr = subkey.to_compact_array();
            let mut compact_buf = BytesMut::with_capacity(33);
            subkey.to_compact(&mut compact_buf);
            assert_eq!(&arr[..], &compact_buf[..], "mismatch at nibble length {len}");
        }
    }

    #[test]
    fn test_packed_stored_nibbles_subkey_compact_array_padding_is_zero() {
        let subkey = PackedStoredNibblesSubKey::from(vec![0x0F; 10]);
        let arr = subkey.to_compact_array();
        assert!(arr[5..32].iter().all(|&b| b == 0), "padding bytes must be zero");
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

    /// Returns `len` nibbles following the pattern `1, 2, ..., f, 0, 1, ...`.
    fn memcmp_test_nibbles(len: usize) -> Nibbles {
        Nibbles::from_iter_unchecked((1..=len).map(|i| (i % 16) as u8))
    }

    fn memcmp_encode(nibbles: Nibbles) -> Vec<u8> {
        let mut buf = Vec::new();
        let len = MemcmpStoredNibblesSubKey(nibbles).to_compact(&mut buf);
        assert_eq!(len, buf.len());
        buf
    }

    fn arb_nibbles() -> impl Strategy<Value = Nibbles> {
        vec(0u8..16, 0..=64).prop_map(Nibbles::from_nibbles_unchecked)
    }

    /// Returns paths over `{0, 1, f}`, mostly zeros, so that paths ending in zero nibbles of the
    /// same group, like `[0]` and `[0, 0]`, are common.
    fn arb_small_alphabet_nibbles() -> impl Strategy<Value = Nibbles> {
        vec(prop_oneof![2 => Just(0u8), 1 => Just(1), 1 => Just(0xf)], 0..=64)
            .prop_map(Nibbles::from_nibbles_unchecked)
    }

    #[test]
    fn test_memcmp_stored_nibbles_exact_bytes() {
        const G1: [u8; 4] = [0x12, 0x34, 0x56, 0x78];
        const G2: [u8; 4] = [0x9A, 0xBC, 0xDE, 0xF0];
        let mut full = Vec::new();
        for group in 0..8 {
            full.extend_from_slice(if group % 2 == 0 { &G1 } else { &G2 });
            full.push(if group == 7 { 8 } else { 9 });
        }

        let cases: Vec<(usize, Vec<u8>)> = vec![
            (0, vec![0x00, 0x00, 0x00, 0x00, 0x00]),
            (1, vec![0x10, 0x00, 0x00, 0x00, 0x01]),
            (2, vec![0x12, 0x00, 0x00, 0x00, 0x02]),
            (7, vec![0x12, 0x34, 0x56, 0x70, 0x07]),
            (8, vec![0x12, 0x34, 0x56, 0x78, 0x08]),
            (9, vec![0x12, 0x34, 0x56, 0x78, 0x09, 0x90, 0x00, 0x00, 0x00, 0x01]),
            (16, vec![0x12, 0x34, 0x56, 0x78, 0x09, 0x9A, 0xBC, 0xDE, 0xF0, 0x08]),
            (
                17,
                vec![
                    0x12, 0x34, 0x56, 0x78, 0x09, 0x9A, 0xBC, 0xDE, 0xF0, 0x09, 0x10, 0x00, 0x00,
                    0x00, 0x01,
                ],
            ),
            (64, full),
        ];

        for (len, expected) in cases {
            let nibbles = memcmp_test_nibbles(len);
            let key = MemcmpStoredNibbles(nibbles);
            let subkey = MemcmpStoredNibblesSubKey(nibbles);

            let mut buf = BytesMut::new();
            assert_eq!(key.to_compact(&mut buf), expected.len(), "length {len}");
            assert_eq!(&buf[..], &expected[..], "key bytes for length {len}");
            assert_eq!(&key.to_compact_array()[..], &expected[..], "key array for length {len}");

            let mut buf = BytesMut::new();
            assert_eq!(subkey.to_compact(&mut buf), expected.len(), "length {len}");
            assert_eq!(&buf[..], &expected[..], "subkey bytes for length {len}");
            assert_eq!(&subkey.to_compact_array()[..], &expected[..], "subkey array {len}");

            let (decoded, rest) = MemcmpStoredNibbles::from_compact(&expected, expected.len());
            assert_eq!(decoded, key);
            assert!(rest.is_empty());
            let (decoded, rest) = MemcmpStoredNibblesSubKey::from_compact(&expected, 0);
            assert_eq!(decoded, subkey);
            assert!(rest.is_empty());
        }
    }

    #[test]
    fn test_memcmp_stored_nibbles_zero_extensions_sort_after_prefix() {
        let encodings: Vec<Vec<u8>> = (0..=64)
            .map(|len| memcmp_encode(Nibbles::from_nibbles_unchecked(vec![0; len])))
            .collect();
        for (len, pair) in encodings.windows(2).enumerate() {
            assert!(pair[0] < pair[1], "zeros of length {len} must sort before length {}", len + 1);
            assert!(!pair[1].starts_with(&pair[0]), "length {len} encoding is a prefix");
        }
    }

    #[test]
    fn test_memcmp_decode_rejects_non_canonical() {
        let mut nine_groups = [0x00, 0x00, 0x00, 0x00, 0x09].repeat(8);
        nine_groups.extend_from_slice(&hex!("0000000001"));
        let cases: [&[u8]; 12] = [
            // Empty input.
            &[],
            // Truncated group, and truncated after a more marker.
            &hex!("12300000"),
            &hex!("123456780990"),
            // More than 8 groups.
            &nine_groups,
            // Final marker out of range.
            &hex!("123456780a"),
            &hex!("12345678ff"),
            // Non-zero padding in the low nibble of a half-used byte, in a trailing byte, and in
            // the empty path.
            &hex!("123f000003"),
            &hex!("1230000103"),
            &hex!("ffffffff00"),
            // Empty group after a more marker.
            &hex!("12345678090000000000"),
            &hex!("1234567809ab00000000"),
            // Non-zero padding in a later group.
            &hex!("12345678091000000f01"),
        ];

        for case in cases {
            assert_eq!(try_decode_memcmp_nibbles(case), None, "{case:?}");
        }
    }

    #[test]
    #[should_panic(expected = "malformed memcmp nibbles encoding")]
    fn test_memcmp_key_from_compact_panics_on_non_canonical() {
        MemcmpStoredNibbles::from_compact(&hex!("123f000003"), 5);
    }

    #[test]
    #[should_panic(expected = "malformed memcmp nibbles encoding")]
    fn test_memcmp_subkey_from_compact_panics_on_non_canonical() {
        MemcmpStoredNibblesSubKey::from_compact(&hex!("123f000003"), 5);
    }

    /// Average encoded size over the mainnet path length distributions from
    /// <https://github.com/paradigmxyz/reth/issues/17843>, versus 33 bytes for the packed encoding.
    #[test]
    fn test_memcmp_mainnet_average_encoded_len() {
        let accounts: &[(usize, u64)] = &[
            (1, 16),
            (2, 256),
            (3, 4096),
            (4, 65536),
            (5, 1048576),
            (6, 16726496),
            (7, 6315318),
            (8, 41590),
            (9, 167),
            (10, 2),
        ];
        let storages: &[(usize, u64)] = &[
            (1, 6502357),
            (2, 17094720),
            (3, 32187105),
            (4, 27030971),
            (5, 25716256),
            (6, 10112317),
            (7, 219679),
            (8, 974),
            (9, 5),
        ];

        for distribution in [accounts, storages] {
            let count: u64 = distribution.iter().map(|(_, count)| count).sum();
            let bytes: u64 = distribution
                .iter()
                .map(|&(len, count)| memcmp_encoded_len(len) as u64 * count)
                .sum();
            let average = bytes as f64 / count as f64;
            assert!(average < 6.0, "average encoded length {average}");
            assert!(average < 33.0 / 6.0, "must be over 6x smaller than packed: {average}");
        }
    }

    proptest! {
        #[test]
        fn proptest_memcmp_roundtrip(nibbles in arb_nibbles(), trailing in vec(any::<u8>(), 0..64)) {
            let mut buf = Vec::new();
            let len = MemcmpStoredNibbles(nibbles).to_compact(&mut buf);
            prop_assert_eq!(len, memcmp_encoded_len(nibbles.len()));
            prop_assert_eq!(&buf[..], &MemcmpStoredNibbles(nibbles).to_compact_array()[..]);
            buf.extend_from_slice(&trailing);

            let (key, rest) = MemcmpStoredNibbles::from_compact(&buf, buf.len());
            prop_assert_eq!(key.0, nibbles);
            prop_assert_eq!(rest, &trailing[..]);

            let (subkey, rest) = MemcmpStoredNibblesSubKey::from_compact(&buf, buf.len());
            prop_assert_eq!(subkey.0, nibbles);
            prop_assert_eq!(rest, &trailing[..]);
        }

        #[test]
        fn proptest_memcmp_order_and_prefix_free(a in arb_nibbles(), b in arb_nibbles()) {
            let (enc_a, enc_b) = (memcmp_encode(a), memcmp_encode(b));
            prop_assert_eq!(a.cmp(&b), enc_a.cmp(&enc_b));
            if a != b {
                prop_assert!(!enc_a.starts_with(&enc_b));
                prop_assert!(!enc_b.starts_with(&enc_a));
            }
        }

        #[test]
        fn proptest_memcmp_order_and_prefix_free_small_alphabet(
            a in arb_small_alphabet_nibbles(),
            b in arb_small_alphabet_nibbles(),
            cut in any::<prop::sample::Index>(),
        ) {
            // Also compares `b` with one of its prefixes, which is often a zero extension.
            let prefix = b.slice(..cut.index(b.len() + 1));
            for (a, b) in [(a, b), (prefix, b)] {
                let (enc_a, enc_b) = (memcmp_encode(a), memcmp_encode(b));
                prop_assert_eq!(a.cmp(&b), enc_a.cmp(&enc_b));
                if a != b {
                    prop_assert!(!enc_a.starts_with(&enc_b));
                    prop_assert!(!enc_b.starts_with(&enc_a));
                }
            }
        }

        #[test]
        fn proptest_memcmp_order_and_prefix_free_shared_prefix(
            prefix in vec(0u8..16, 0..=64),
            suffix_a in vec(0u8..16, 0..=64),
            suffix_b in vec(0u8..16, 0..=64),
        ) {
            let path = |suffix: &[u8]| {
                let mut path = [&prefix[..], suffix].concat();
                path.truncate(64);
                Nibbles::from_nibbles_unchecked(path)
            };
            let (a, b) = (path(&suffix_a), path(&suffix_b));
            let (enc_a, enc_b) = (memcmp_encode(a), memcmp_encode(b));
            prop_assert_eq!(a.cmp(&b), enc_a.cmp(&enc_b));
            if a != b {
                prop_assert!(!enc_a.starts_with(&enc_b));
                prop_assert!(!enc_b.starts_with(&enc_a));
            }
        }
    }
}
