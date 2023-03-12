use derive_more::{AsRef, Deref};
use fixed_hash::{construct_fixed_hash, impl_fixed_hash_conversions};

#[cfg(any(test, feature = "arbitrary"))]
use arbitrary::Arbitrary;
#[cfg(any(test, feature = "arbitrary"))]
use proptest_derive::Arbitrary as PropTestArbitrary;

construct_fixed_hash! {
    /// revm 256 bits type.
    #[cfg_attr(any(test, feature = "arbitrary"), derive(Arbitrary, PropTestArbitrary))]
    #[derive(AsRef,Deref)]
    pub struct B256(32);
}

construct_fixed_hash! {
    /// revm 160 bits type.
    #[cfg_attr(any(test, feature = "arbitrary"), derive(Arbitrary, PropTestArbitrary))]
    #[derive(AsRef,Deref)]
    pub struct B160(20);
}

impl From<u64> for B160 {
    fn from(fr: u64) -> Self {
        let x_bytes = fr.to_be_bytes();
        B160([
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, x_bytes[0], x_bytes[1], x_bytes[2], x_bytes[3],
            x_bytes[4], x_bytes[5], x_bytes[6], x_bytes[7],
        ])
    }
}

#[cfg(feature = "primitive-types")]
impl From<primitive_types::H160> for B160 {
    fn from(fr: primitive_types::H160) -> Self {
        B160(fr.0)
    }
}

#[cfg(feature = "primitive-types")]
impl From<primitive_types::H256> for B256 {
    fn from(fr: primitive_types::H256) -> Self {
        B256(fr.0)
    }
}

#[cfg(feature = "primitive-types")]
impl From<B160> for primitive_types::H160 {
    fn from(fr: B160) -> Self {
        primitive_types::H160(fr.0)
    }
}

#[cfg(feature = "primitive-types")]
impl From<B256> for primitive_types::H256 {
    fn from(fr: B256) -> Self {
        primitive_types::H256(fr.0)
    }
}

#[cfg(feature = "primitive-types")]
impl From<primitive_types::U256> for B256 {
    fn from(fr: primitive_types::U256) -> Self {
        let mut ret = B256::zero();
        fr.to_big_endian(ret.as_bytes_mut());
        ret
    }
}

#[cfg(feature = "primitive-types")]
impl From<B256> for primitive_types::U256 {
    fn from(fr: B256) -> Self {
        primitive_types::U256::from(fr.as_ref() as &[u8])
    }
}

impl From<ruint::aliases::U256> for B256 {
    fn from(fr: ruint::aliases::U256) -> Self {
        B256(fr.to_be_bytes())
    }
}

impl From<B256> for ruint::aliases::U256 {
    fn from(fr: B256) -> Self {
        ruint::aliases::U256::from_be_bytes(fr.0)
    }
}

impl_fixed_hash_conversions!(B256, B160);

#[cfg(feature = "serde")]
impl serde::Serialize for B256 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut slice = [0u8; 2 + 2 * 32];
        serialize::serialize_raw(&mut slice, &self.0, serializer)
    }
}
#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for B256 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let mut bytes = [0u8; 32];
        serialize::deserialize_check_len(deserializer, serialize::ExpectedLen::Exact(&mut bytes))?;
        Ok(B256(bytes))
    }
}
#[cfg(feature = "serde")]
impl serde::Serialize for B160 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut slice = [0u8; 2 + 2 * 20];
        serialize::serialize_raw(&mut slice, &self.0, serializer)
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for B160 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let mut bytes = [0u8; 20];
        serialize::deserialize_check_len(deserializer, serialize::ExpectedLen::Exact(&mut bytes))?;
        Ok(B160(bytes))
    }
}

// code optained from: https://docs.rs/impl-serde/0.4.0/impl_serde/
#[cfg(feature = "serde")]
mod serialize {

    use alloc::string::String;
    use core::{fmt, result::Result};
    use serde::{de, Deserializer, Serializer};

    static CHARS: &[u8] = b"0123456789abcdef";

    fn to_hex_raw<'a>(v: &'a mut [u8], bytes: &[u8], skip_leading_zero: bool) -> &'a str {
        assert!(v.len() > 1 + bytes.len() * 2);

        v[0] = b'0';
        v[1] = b'x';

        let mut idx = 2;
        let first_nibble = bytes[0] >> 4;
        if first_nibble != 0 || !skip_leading_zero {
            v[idx] = CHARS[first_nibble as usize];
            idx += 1;
        }
        v[idx] = CHARS[(bytes[0] & 0xf) as usize];
        idx += 1;

        for &byte in bytes.iter().skip(1) {
            v[idx] = CHARS[(byte >> 4) as usize];
            v[idx + 1] = CHARS[(byte & 0xf) as usize];
            idx += 2;
        }

        // SAFETY: all characters come either from CHARS or "0x", therefore valid UTF8
        unsafe { core::str::from_utf8_unchecked(&v[0..idx]) }
    }

    /// Decoding bytes from hex string error.
    #[derive(Debug, PartialEq, Eq)]
    pub(crate) enum FromHexError {
        /// Invalid (non-hex) character encountered.
        InvalidHex {
            /// The unexpected character.
            character: char,
            /// Index of that occurrence.
            index: usize,
        },
    }

    #[cfg(feature = "std")]
    impl std::error::Error for FromHexError {}

    impl fmt::Display for FromHexError {
        fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
            match *self {
                Self::InvalidHex { character, index } => {
                    write!(fmt, "invalid hex character: {character}, at {index}")
                }
            }
        }
    }

    /// Decode given 0x-prefix-stripped hex string into provided slice.
    /// Used internally by `from_hex` and `deserialize_check_len`.
    ///
    /// The method will panic if `bytes` have incorrect length (make sure to allocate enough
    /// beforehand).
    fn from_hex_raw(v: &str, bytes: &mut [u8], stripped: bool) -> Result<usize, FromHexError> {
        let bytes_len = v.len();
        let mut modulus = bytes_len % 2;
        let mut buf = 0;
        let mut pos = 0;
        for (index, byte) in v.bytes().enumerate() {
            buf <<= 4;

            match byte {
                b'A'..=b'F' => buf |= byte - b'A' + 10,
                b'a'..=b'f' => buf |= byte - b'a' + 10,
                b'0'..=b'9' => buf |= byte - b'0',
                b' ' | b'\r' | b'\n' | b'\t' => {
                    buf >>= 4;
                    continue
                }
                b => {
                    let character = char::from(b);
                    return Err(FromHexError::InvalidHex {
                        character,
                        index: index + if stripped { 2 } else { 0 },
                    })
                }
            }

            modulus += 1;
            if modulus == 2 {
                modulus = 0;
                bytes[pos] = buf;
                pos += 1;
            }
        }

        Ok(pos)
    }

    /// Serializes a slice of bytes.
    pub(crate) fn serialize_raw<S>(
        slice: &mut [u8],
        bytes: &[u8],
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if bytes.is_empty() {
            serializer.serialize_str("0x")
        } else {
            serializer.serialize_str(to_hex_raw(slice, bytes, false))
        }
    }

    /// Expected length of bytes vector.
    #[derive(Debug, PartialEq, Eq)]
    pub(crate) enum ExpectedLen<'a> {
        /// Exact length in bytes.
        Exact(&'a mut [u8]),
        /// A bytes length between (min; slice.len()].
        #[allow(dead_code)]
        Between(usize, &'a mut [u8]),
    }

    impl<'a> fmt::Display for ExpectedLen<'a> {
        fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
            match *self {
                ExpectedLen::Exact(ref v) => write!(fmt, "length of {}", v.len() * 2),
                ExpectedLen::Between(min, ref v) => {
                    write!(fmt, "length between ({}; {}]", min * 2, v.len() * 2)
                }
            }
        }
    }

    /// Deserialize into vector of bytes with additional size check.
    /// Returns number of bytes written.
    pub(crate) fn deserialize_check_len<'a, 'de, D>(
        deserializer: D,
        len: ExpectedLen<'a>,
    ) -> Result<usize, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct Visitor<'a> {
            len: ExpectedLen<'a>,
        }

        impl<'a, 'b> de::Visitor<'b> for Visitor<'a> {
            type Value = usize;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(formatter, "a (both 0x-prefixed or not) hex string with {}", self.len)
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                let (v, stripped) = v.strip_prefix("0x").map_or((v, false), |v| (v, true));

                let len = v.len();
                let is_len_valid = match self.len {
                    ExpectedLen::Exact(ref slice) => len == 2 * slice.len(),
                    ExpectedLen::Between(min, ref slice) => len <= 2 * slice.len() && len > 2 * min,
                };

                if !is_len_valid {
                    return Err(E::invalid_length(v.len(), &self))
                }

                let bytes = match self.len {
                    ExpectedLen::Exact(slice) => slice,
                    ExpectedLen::Between(_, slice) => slice,
                };

                from_hex_raw(v, bytes, stripped).map_err(E::custom)
            }

            fn visit_string<E: de::Error>(self, v: String) -> Result<Self::Value, E> {
                self.visit_str(&v)
            }
        }

        deserializer.deserialize_str(Visitor { len })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arbitrary() {
        proptest::proptest!(|(_v1: B160, _v2: B256)| {});
    }

    #[test]
    fn should_convert_from_primitive_types_h160() {
        let h160 = primitive_types::H160::random();
        let b160: B160 = h160.into();
        let new_h160: primitive_types::H160 = b160.into();
        assert_eq!(h160, new_h160)
    }

    #[test]
    fn should_convert_to_primitive_types_h160() {
        let b160 = B160::random();
        let h160: primitive_types::H160 = b160.into();
        let new_b160: B160 = h160.into();
        assert_eq!(b160, new_b160)
    }

    #[test]
    fn should_convert_from_primitive_types_h256() {
        let h256 = primitive_types::H256::random();
        let b256: B256 = h256.into();
        let new_h256: primitive_types::H256 = b256.into();
        assert_eq!(h256, new_h256)
    }

    #[test]
    fn should_convert_to_primitive_types_h256() {
        let b256 = B256::random();
        let h256: primitive_types::H256 = b256.into();
        let new_b256: B256 = h256.into();
        assert_eq!(b256, new_b256)
    }

    #[test]
    fn should_convert_to_primitive_types_h256_u256() {
        let b256 = B256::random();
        let u256: primitive_types::U256 = b256.into();
        let new_b256: B256 = u256.into();
        assert_eq!(b256, new_b256)
    }

    #[test]
    fn should_convert_to_ruint_u256() {
        let b256 = B256::random();
        let u256: ruint::aliases::U256 = b256.into();
        let new_b256: B256 = u256.into();
        assert_eq!(b256, new_b256)
    }
}
