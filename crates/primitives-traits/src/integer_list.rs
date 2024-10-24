use alloc::vec::Vec;
use bytes::BufMut;
use core::fmt;
use derive_more::Deref;
use roaring::RoaringTreemap;
use serde::{
    de::{SeqAccess, Visitor},
    ser::SerializeSeq,
    Deserialize, Deserializer, Serialize, Serializer,
};

/// Uses Roaring Bitmaps to hold a list of integers. It provides really good compression with the
/// capability to access its elements without decoding it.
#[derive(Clone, PartialEq, Default, Deref)]
pub struct IntegerList(pub RoaringTreemap);

impl fmt::Debug for IntegerList {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("IntegerList")?;
        f.debug_list().entries(self.0.iter()).finish()
    }
}

impl IntegerList {
    /// Creates a new empty `IntegerList`.
    pub fn empty() -> Self {
        Self(RoaringTreemap::new())
    }

    /// Creates an `IntegerList` from a list of integers.
    ///
    /// Returns an error if the list is not pre-sorted.
    pub fn new(list: impl IntoIterator<Item = u64>) -> Result<Self, IntegerListError> {
        RoaringTreemap::from_sorted_iter(list)
            .map(Self)
            .map_err(|_| IntegerListError::UnsortedInput)
    }

    // Creates an IntegerList from a pre-sorted list of integers.
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
        if self.0.push(value) {
            Ok(())
        } else {
            Err(IntegerListError::UnsortedInput)
        }
    }

    /// Clears the list.
    pub fn clear(&mut self) {
        self.0.clear();
    }

    /// Serializes a [`IntegerList`] into a sequence of bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut vec = Vec::with_capacity(self.0.serialized_size());
        self.0.serialize_into(&mut vec).expect("not able to encode IntegerList");
        vec
    }

    /// Serializes a [`IntegerList`] into a sequence of bytes.
    pub fn to_mut_bytes<B: bytes::BufMut>(&self, buf: &mut B) {
        self.0.serialize_into(buf.writer()).unwrap();
    }

    /// Deserializes a sequence of bytes into a proper [`IntegerList`].
    pub fn from_bytes(data: &[u8]) -> Result<Self, IntegerListError> {
        Ok(Self(
            RoaringTreemap::deserialize_from(data)
                .map_err(|_| IntegerListError::FailedToDeserialize)?,
        ))
    }
}

impl Serialize for IntegerList {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut seq = serializer.serialize_seq(Some(self.len() as usize))?;
        for e in &self.0 {
            seq.serialize_element(&e)?;
        }
        seq.end()
    }
}

struct IntegerListVisitor;
impl<'de> Visitor<'de> for IntegerListVisitor {
    type Value = IntegerList;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a usize array")
    }

    fn visit_seq<E>(self, mut seq: E) -> Result<Self::Value, E::Error>
    where
        E: SeqAccess<'de>,
    {
        let mut list = IntegerList::empty();
        while let Some(item) = seq.next_element()? {
            list.push(item).map_err(serde::de::Error::custom)?;
        }
        Ok(list)
    }
}

impl<'de> Deserialize<'de> for IntegerList {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
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
    fn serde_serialize_deserialize() {
        let original_list = [1, 2, 3];
        let ef_list = IntegerList::new(original_list).unwrap();

        let serde_out = serde_json::to_string(&ef_list).unwrap();
        let serde_ef_list = serde_json::from_str::<IntegerList>(&serde_out).unwrap();
        assert_eq!(serde_ef_list, ef_list);
    }
}
