//! Implements [`Compress`] and [`Decompress`] for [`IntegerList`]

use crate::table::{Compress, Decompress};
use bytes::BufMut;
use core::{fmt, ops::RangeBounds};
use derive_more::Deref;
use reth_codecs::DecompressError;
use roaring::RoaringTreemap;

/// A data structure that uses Roaring Bitmaps to efficiently store a list of integers.
///
/// This structure provides excellent compression while allowing direct access to individual
/// elements without the need for full decompression.
///
/// Key features:
/// - Efficient compression: the underlying Roaring Bitmaps significantly reduce memory usage.
/// - Direct access: elements can be accessed or queried without needing to decode the entire list.
/// - [`RoaringTreemap`] backing: internally backed by [`RoaringTreemap`], which supports 64-bit
///   integers.
#[derive(Clone, PartialEq, Eq, Default, Deref)]
pub struct IntegerList(pub RoaringTreemap);

impl fmt::Debug for IntegerList {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("IntegerList")?;
        f.debug_list().entries(self.0.iter()).finish()
    }
}

impl IntegerList {
    /// Creates a new empty [`IntegerList`].
    pub fn empty() -> Self {
        Self(RoaringTreemap::new())
    }

    /// Creates an [`IntegerList`] from a list of integers.
    ///
    /// Returns an error if the list is not pre-sorted.
    pub fn new(list: impl IntoIterator<Item = u64>) -> Result<Self, IntegerListError> {
        RoaringTreemap::from_sorted_iter(list)
            .map(Self)
            .map_err(|_| IntegerListError::UnsortedInput)
    }

    /// Creates an [`IntegerList`] from a pre-sorted list of integers.
    ///
    /// # Panics
    ///
    /// Panics if the list is not pre-sorted.
    #[inline]
    #[track_caller]
    pub fn new_pre_sorted(list: impl IntoIterator<Item = u64>) -> Self {
        Self::new(list).expect("IntegerList must be pre-sorted and non-empty")
    }

    /// Appends a list of integers to the current list.
    pub fn append(&mut self, list: impl IntoIterator<Item = u64>) -> Result<u64, IntegerListError> {
        self.0.append(list).map_err(|_| IntegerListError::UnsortedInput)
    }

    /// Pushes a new integer to the list.
    pub fn push(&mut self, value: u64) -> Result<(), IntegerListError> {
        self.0.try_push(value).map_err(|_| IntegerListError::UnsortedInput)
    }

    /// Clears the list.
    pub fn clear(&mut self) {
        self.0.clear();
    }

    /// Removes the integers in the given range, returning how many were removed.
    pub fn remove_range<R: RangeBounds<u64>>(&mut self, range: R) -> u64 {
        self.0.remove_range(range)
    }

    /// Serializes an [`IntegerList`] into a sequence of bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut vec = Vec::with_capacity(self.0.serialized_size());
        self.0.serialize_into(&mut vec).expect("not able to encode IntegerList");
        vec
    }

    /// Serializes an [`IntegerList`] into a sequence of bytes.
    pub fn to_mut_bytes<B: bytes::BufMut>(&self, buf: &mut B) {
        self.0.serialize_into(buf.writer()).unwrap();
    }

    /// Deserializes a sequence of bytes into a proper [`IntegerList`].
    pub fn from_bytes(data: &[u8]) -> Result<Self, IntegerListError> {
        RoaringTreemap::deserialize_from(data)
            .map(Self)
            .map_err(|_| IntegerListError::FailedToDeserialize)
    }
}

impl serde::Serialize for IntegerList {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeSeq;

        let mut seq = serializer.serialize_seq(Some(self.len() as usize))?;
        for e in &self.0 {
            seq.serialize_element(&e)?;
        }
        seq.end()
    }
}

struct IntegerListVisitor;

impl<'de> serde::de::Visitor<'de> for IntegerListVisitor {
    type Value = IntegerList;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a usize array")
    }

    fn visit_seq<E>(self, mut seq: E) -> Result<Self::Value, E::Error>
    where
        E: serde::de::SeqAccess<'de>,
    {
        let mut list = IntegerList::empty();
        while let Some(item) = seq.next_element()? {
            list.push(item).map_err(serde::de::Error::custom)?;
        }
        Ok(list)
    }
}

impl<'de> serde::Deserialize<'de> for IntegerList {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_byte_buf(IntegerListVisitor)
    }
}

#[cfg(any(test, feature = "arbitrary"))]
use arbitrary::{Arbitrary, Unstructured};

#[cfg(any(test, feature = "arbitrary"))]
impl<'a> Arbitrary<'a> for IntegerList {
    fn arbitrary(u: &mut Unstructured<'a>) -> Result<Self, arbitrary::Error> {
        let mut nums: Vec<u64> = Vec::arbitrary(u)?;
        nums.sort_unstable();
        Self::new(nums).map_err(|_| arbitrary::Error::IncorrectFormat)
    }
}

/// Primitives error type.
#[derive(Debug, derive_more::Display, derive_more::Error)]
pub enum IntegerListError {
    /// The provided input is unsorted.
    #[display("the provided input is unsorted")]
    UnsortedInput,
    /// Failed to deserialize data into type.
    #[display("failed to deserialize data into type")]
    FailedToDeserialize,
}

impl Compress for IntegerList {
    type Compressed = Vec<u8>;

    fn compress(self) -> Self::Compressed {
        self.to_bytes()
    }

    fn compress_to_buf<B: bytes::BufMut + AsMut<[u8]>>(&self, buf: &mut B) {
        self.to_mut_bytes(buf)
    }
}

impl Decompress for IntegerList {
    fn decompress(value: &[u8]) -> Result<Self, DecompressError> {
        Self::from_bytes(value).map_err(DecompressError::new)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_list() {
        assert_eq!(IntegerList::empty().len(), 0);
        assert_eq!(IntegerList::new_pre_sorted(std::iter::empty()).len(), 0);
    }

    #[test]
    fn test_integer_list() {
        let original_list = [1, 2, 3];
        let ef_list = IntegerList::new(original_list).unwrap();
        assert_eq!(ef_list.iter().collect::<Vec<_>>(), original_list);
    }

    #[test]
    fn test_integer_list_serialization() {
        let original_list = [1, 2, 3];
        let ef_list = IntegerList::new(original_list).unwrap();

        let blist = ef_list.to_bytes();
        assert_eq!(IntegerList::from_bytes(&blist).unwrap(), ef_list)
    }

    #[test]
    fn remove_range_matches_filtering() {
        // Spans more than one 2^16 roaring container so multi-container removal is covered.
        let values = [1u64, 2, 100, 65_535, 65_536, 70_000, 200_000];

        for to_block in [0u64, 1, 99, 100, 65_535, 65_536, 199_999, 200_000, 200_001] {
            let mut list = IntegerList::new(values).unwrap();
            let removed = list.remove_range(0..=to_block);

            let expected = values.into_iter().filter(|value| *value > to_block).collect::<Vec<_>>();
            assert_eq!(list.iter().collect::<Vec<_>>(), expected, "to_block {to_block}");
            assert_eq!(removed, (values.len() - expected.len()) as u64, "to_block {to_block}");
            assert_eq!(list.is_empty(), expected.is_empty(), "to_block {to_block}");
        }
    }

    #[test]
    fn remove_range_on_empty_list_removes_nothing() {
        let mut list = IntegerList::empty();
        assert_eq!(list.remove_range(0..=100), 0);
        assert!(list.is_empty());
    }
}
