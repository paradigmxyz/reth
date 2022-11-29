use reth_codecs::{main_codec, Compact};
use reth_rlp::{Decodable, DecodeError, Encodable};
use serde::{Deserialize, Deserializer, Serializer};
use std::{
    borrow::Borrow,
    clone::Clone,
    fmt::{Debug, Display, Formatter, LowerHex, Result as FmtResult},
    ops::Deref,
    str::FromStr,
};
use thiserror::Error;

/// Wrapper type around Bytes to deserialize/serialize "0x" prefixed ethereum hex strings
#[main_codec]
#[derive(Clone, Default, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct Bytes(
    #[serde(serialize_with = "serialize_bytes", deserialize_with = "deserialize_bytes")]
    pub  bytes::Bytes,
);

fn bytes_to_hex(b: &Bytes) -> String {
    hex::encode(b.0.as_ref())
}

impl Debug for Bytes {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(f, "Bytes(0x{})", bytes_to_hex(self))
    }
}

impl Display for Bytes {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(f, "0x{}", bytes_to_hex(self))
    }
}

impl LowerHex for Bytes {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(f, "0x{}", bytes_to_hex(self))
    }
}

impl Bytes {
    /// Return bytes as [Vec::<u8>]
    pub fn to_vec(&self) -> Vec<u8> {
        self.as_ref().to_vec()
    }
}

impl Deref for Bytes {
    type Target = [u8];

    #[inline]
    fn deref(&self) -> &[u8] {
        self.as_ref()
    }
}

impl AsRef<[u8]> for Bytes {
    fn as_ref(&self) -> &[u8] {
        self.0.as_ref()
    }
}

impl Borrow<[u8]> for Bytes {
    fn borrow(&self) -> &[u8] {
        self.as_ref()
    }
}

impl IntoIterator for Bytes {
    type Item = u8;
    type IntoIter = bytes::buf::IntoIter<bytes::Bytes>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a> IntoIterator for &'a Bytes {
    type Item = &'a u8;
    type IntoIter = core::slice::Iter<'a, u8>;

    fn into_iter(self) -> Self::IntoIter {
        self.as_ref().iter()
    }
}

impl From<bytes::Bytes> for Bytes {
    fn from(src: bytes::Bytes) -> Self {
        Self(src)
    }
}

impl From<Vec<u8>> for Bytes {
    fn from(src: Vec<u8>) -> Self {
        Self(src.into())
    }
}

impl<const N: usize> From<[u8; N]> for Bytes {
    fn from(src: [u8; N]) -> Self {
        src.to_vec().into()
    }
}

impl<'a, const N: usize> From<&'a [u8; N]> for Bytes {
    fn from(src: &'a [u8; N]) -> Self {
        src.to_vec().into()
    }
}

impl PartialEq<[u8]> for Bytes {
    fn eq(&self, other: &[u8]) -> bool {
        self.as_ref() == other
    }
}

impl PartialEq<Bytes> for [u8] {
    fn eq(&self, other: &Bytes) -> bool {
        *other == *self
    }
}

impl PartialEq<Vec<u8>> for Bytes {
    fn eq(&self, other: &Vec<u8>) -> bool {
        self.as_ref() == &other[..]
    }
}

impl PartialEq<Bytes> for Vec<u8> {
    fn eq(&self, other: &Bytes) -> bool {
        *other == *self
    }
}

impl PartialEq<bytes::Bytes> for Bytes {
    fn eq(&self, other: &bytes::Bytes) -> bool {
        other == self.as_ref()
    }
}

impl Encodable for Bytes {
    fn length(&self) -> usize {
        self.0.length()
    }
    fn encode(&self, out: &mut dyn bytes::BufMut) {
        self.0.encode(out)
    }
}

impl Decodable for Bytes {
    fn decode(buf: &mut &[u8]) -> Result<Self, DecodeError> {
        Ok(Self(bytes::Bytes::decode(buf)?))
    }
}

#[derive(Debug, Clone, Error)]
#[error("Failed to parse bytes: {0}")]
pub struct ParseBytesError(String);

impl FromStr for Bytes {
    type Err = ParseBytesError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if let Some(value) = value.strip_prefix("0x") {
            hex::decode(value)
        } else {
            hex::decode(value)
        }
        .map(Into::into)
        .map_err(|e| ParseBytesError(format!("Invalid hex: {}", e)))
    }
}

fn serialize_bytes<S, T>(x: T, s: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
    T: AsRef<[u8]>,
{
    s.serialize_str(&format!("0x{}", hex::encode(x.as_ref())))
}

fn deserialize_bytes<'de, D>(d: D) -> Result<bytes::Bytes, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(d)?;
    if let Some(value) = value.strip_prefix("0x") {
        hex::decode(value)
    } else {
        hex::decode(&value)
    }
    .map(Into::into)
    .map_err(|e| serde::de::Error::custom(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_formatting() {
        let b = Bytes::from(vec![1, 35, 69, 103, 137, 171, 205, 239]);
        let expected = String::from("0x0123456789abcdef");
        assert_eq!(format!("{:x}", b), expected);
        assert_eq!(format!("{}", b), expected);
    }

    #[test]
    fn test_from_str() {
        let b = Bytes::from_str("0x1213");
        assert!(b.is_ok());
        let b = b.unwrap();
        assert_eq!(b.as_ref(), hex::decode("1213").unwrap());

        let b = Bytes::from_str("1213");
        let b = b.unwrap();
        assert_eq!(b.as_ref(), hex::decode("1213").unwrap());
    }

    #[test]
    fn test_debug_formatting() {
        let b = Bytes::from(vec![1, 35, 69, 103, 137, 171, 205, 239]);
        assert_eq!(format!("{:?}", b), "Bytes(0x0123456789abcdef)");
        assert_eq!(format!("{:#?}", b), "Bytes(0x0123456789abcdef)");
    }
}
