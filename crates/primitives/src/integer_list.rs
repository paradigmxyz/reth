use serde::{
    de::{SeqAccess, Unexpected, Visitor},
    ser::SerializeSeq,
    Deserialize, Deserializer, Serialize, Serializer,
};
use std::{fmt, ops::Deref};
use sucds::{EliasFano, Searial};

/// Uses EliasFano to hold a list of integers. It provides really good compression with the
/// capability to access its elements without decoding it.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct IntegerList(pub EliasFano);

impl Deref for IntegerList {
    type Target = EliasFano;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl fmt::Debug for IntegerList {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let vec: Vec<usize> = self.0.iter(0).collect();
        write!(f, "IntegerList {:?}", vec)
    }
}

impl IntegerList {
    /// Creates an IntegerList from a list of integers. `usize` is safe to use since
    /// [`sucds::EliasFano`] restricts its compilation to 64bits.
    ///
    /// # Returns
    ///
    /// Returns an error if the list is empty or not pre-sorted.
    pub fn new<T: AsRef<[usize]>>(list: T) -> Result<Self, EliasFanoError> {
        Ok(Self(EliasFano::from_ints(list.as_ref()).map_err(|_| EliasFanoError::InvalidInput)?))
    }

    // Creates an IntegerList from a pre-sorted list of integers. `usize` is safe to use since
    /// [`sucds::EliasFano`] restricts its compilation to 64bits.
    ///
    /// # Panics
    ///
    /// Panics if the list is empty or not pre-sorted.
    pub fn new_pre_sorted<T: AsRef<[usize]>>(list: T) -> Self {
        Self(
            EliasFano::from_ints(list.as_ref())
                .expect("IntegerList must be pre-sorted and non-empty."),
        )
    }

    /// Serializes a [`IntegerList`] into a sequence of bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut vec = Vec::with_capacity(self.0.size_in_bytes());
        self.0.serialize_into(&mut vec).expect("not able to encode integer list.");
        vec
    }

    /// Serializes a [`IntegerList`] into a sequence of bytes.
    pub fn to_mut_bytes<B: bytes::BufMut>(&self, buf: &mut B) {
        let len = self.0.size_in_bytes();
        let mut vec = Vec::with_capacity(len);
        self.0.serialize_into(&mut vec).unwrap();
        buf.put_slice(vec.as_slice());
    }

    /// Deserializes a sequence of bytes into a proper [`IntegerList`].
    pub fn from_bytes(data: &[u8]) -> Result<Self, EliasFanoError> {
        Ok(Self(EliasFano::deserialize_from(data).map_err(|_| EliasFanoError::FailedDeserialize)?))
    }
}

macro_rules! impl_uint {
    ($($w:tt),+) => {
        $(
            impl From<Vec<$w>> for IntegerList {
                fn from(v: Vec<$w>) -> Self {
                    let v: Vec<usize> = v.iter().map(|v| *v as usize).collect();
                    Self(EliasFano::from_ints(v.as_slice()).expect("could not create list."))
                }
            }
        )+
    };
}

impl_uint!(usize, u64, u32, u8, u16);

impl Serialize for IntegerList {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let vec = self.0.iter(0).collect::<Vec<usize>>();
        let mut seq = serializer.serialize_seq(Some(self.len()))?;
        for e in vec {
            seq.serialize_element(&e)?;
        }
        seq.end()
    }
}

struct IntegerListVisitor;
impl<'de> Visitor<'de> for IntegerListVisitor {
    type Value = IntegerList;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a usize array")
    }

    fn visit_seq<E>(self, mut seq: E) -> Result<Self::Value, E::Error>
    where
        E: SeqAccess<'de>,
    {
        let mut list = Vec::new();
        while let Some(item) = seq.next_element()? {
            list.push(item);
        }

        IntegerList::new(list).map_err(|_| serde::de::Error::invalid_value(Unexpected::Seq, &self))
    }
}

impl<'de> Deserialize<'de> for IntegerList {
    fn deserialize<D>(deserializer: D) -> Result<IntegerList, D::Error>
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
        let mut nums: Vec<usize> = Vec::arbitrary(u)?;
        nums.sort();
        Ok(Self(EliasFano::from_ints(&nums).map_err(|_| arbitrary::Error::IncorrectFormat)?))
    }
}

/// Primitives error type.
#[derive(Debug, thiserror::Error)]
pub enum EliasFanoError {
    /// The provided input is invalid.
    #[error("the provided input is invalid")]
    InvalidInput,
    /// Failed to deserialize data into type.
    #[error("failed to deserialize data into type")]
    FailedDeserialize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_integer_list() {
        let original_list = [1, 2, 3];
        let ef_list = IntegerList::new(original_list).unwrap();
        assert_eq!(ef_list.iter(0).collect::<Vec<usize>>(), original_list);
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
