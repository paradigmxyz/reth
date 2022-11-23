use crate::error::Error;
use serde::{
    de::{Unexpected, Visitor},
    Deserialize, Deserializer, Serialize, Serializer,
};
use std::ops::Deref;
use sucds::{EliasFano, Searial};

/// Uses EliasFano to hold a list of integers. It provides really good compression with the
/// capability to access its elements without decoding it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IntegerList(pub EliasFano);

impl Deref for IntegerList {
    type Target = EliasFano;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl IntegerList {
    /// Creates an IntegerList from a list of integers. `usize` is safe to use since
    /// [`sucds::EliasFano`] restricts its compilation to 64bits.
    ///
    /// List should be pre-sorted and not empty.
    pub fn new<T: AsRef<[usize]>>(list: T) -> Result<Self, Error> {
        Ok(Self(EliasFano::from_ints(list.as_ref()).map_err(|_| Error::InvalidInput)?))
    }

    /// Serializes a [`IntegerList`] into a sequence of bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut vec = Vec::with_capacity(self.0.size_in_bytes());
        self.0.serialize_into(&mut vec).expect("not able to encode integer list.");
        vec
    }

    /// Deserializes a sequence of bytes into a proper [`IntegerList`].
    pub fn from_bytes(data: &[u8]) -> Result<Self, Error> {
        Ok(Self(EliasFano::deserialize_from(data).map_err(|_| Error::FailedDeserialize)?))
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
        serializer.serialize_bytes(&self.to_bytes())
    }
}

struct IntegerListVisitor;
impl<'de> Visitor<'de> for IntegerListVisitor {
    type Value = IntegerList;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a byte array")
    }

    fn visit_bytes<E>(self, v: &[u8]) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        IntegerList::from_bytes(v)
            .map_err(|_| serde::de::Error::invalid_type(Unexpected::Bytes(v), &self))
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

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_integer_list() {
        let original_list = [1, 2, 3];

        let ef_list = IntegerList::new(original_list).unwrap();

        assert!(ef_list.iter(0).collect::<Vec<usize>>() == original_list);
    }

    #[test]
    fn test_integer_list_serialization() {
        let original_list = [1, 2, 3];
        let ef_list = IntegerList::new(original_list).unwrap();

        let blist = ef_list.to_bytes();
        assert!(IntegerList::from_bytes(&blist).unwrap() == ef_list)
    }
}
