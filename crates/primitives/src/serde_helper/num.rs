//! Numeric helpers

use crate::{U256, U64};
use serde::{de, Deserialize, Deserializer, Serialize};
use std::str::FromStr;

/// A `u64` wrapper type that deserializes from hex or a u64 and serializes as hex.
///
///
/// ```rust
/// use reth_primitives::serde_helper::num::U64HexOrNumber;
/// let number_json = "100";
/// let hex_json = "\"0x64\"";
///
/// let number: U64HexOrNumber = serde_json::from_str(number_json).unwrap();
/// let hex: U64HexOrNumber = serde_json::from_str(hex_json).unwrap();
/// assert_eq!(number, hex);
/// assert_eq!(hex.as_u64(), 100);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct U64HexOrNumber(U64);

impl U64HexOrNumber {
    /// Returns the wrapped u64
    pub fn as_u64(self) -> u64 {
        self.0.as_u64()
    }
}

impl From<u64> for U64HexOrNumber {
    fn from(value: u64) -> Self {
        Self(U64::from(value))
    }
}

impl From<U64> for U64HexOrNumber {
    fn from(value: U64) -> Self {
        Self(value)
    }
}

impl From<U64HexOrNumber> for u64 {
    fn from(value: U64HexOrNumber) -> Self {
        value.as_u64()
    }
}

impl From<U64HexOrNumber> for U64 {
    fn from(value: U64HexOrNumber) -> Self {
        value.0
    }
}

impl<'de> Deserialize<'de> for U64HexOrNumber {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum NumberOrHexU64 {
            Hex(U64),
            Int(u64),
        }
        match NumberOrHexU64::deserialize(deserializer)? {
            NumberOrHexU64::Int(val) => Ok(val.into()),
            NumberOrHexU64::Hex(val) => Ok(val.into()),
        }
    }
}

/// serde functions for handling primitive `u64` as [U64](crate::U64)
pub mod u64_hex_or_decimal {
    use crate::serde_helper::num::U64HexOrNumber;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    /// Deserializes an `u64` accepting a hex quantity string with optional 0x prefix or
    /// a number
    pub fn deserialize<'de, D>(deserializer: D) -> Result<u64, D::Error>
    where
        D: Deserializer<'de>,
    {
        U64HexOrNumber::deserialize(deserializer).map(Into::into)
    }

    /// Serializes u64 as hex string
    pub fn serialize<S: Serializer>(value: &u64, s: S) -> Result<S::Ok, S::Error> {
        U64HexOrNumber::from(*value).serialize(s)
    }
}
/// Deserializes the input into an `Option<U256>`, using [`from_int_or_hex`] to deserialize the
/// inner value.
pub fn from_int_or_hex_opt<'de, D>(deserializer: D) -> Result<Option<U256>, D::Error>
where
    D: Deserializer<'de>,
{
    match Option::<NumberOrHexU256>::deserialize(deserializer)? {
        Some(val) => val.try_into_u256().map(Some),
        None => Ok(None),
    }
}

/// Deserializes the input into a U256, accepting both 0x-prefixed hex and decimal strings with
/// arbitrary precision, defined by serde_json's [`Number`](serde_json::Number).
pub fn from_int_or_hex<'de, D>(deserializer: D) -> Result<U256, D::Error>
where
    D: Deserializer<'de>,
{
    NumberOrHexU256::deserialize(deserializer)?.try_into_u256()
}

#[derive(Deserialize)]
#[serde(untagged)]
enum NumberOrHexU256 {
    Int(serde_json::Number),
    Hex(U256),
}

impl NumberOrHexU256 {
    fn try_into_u256<E: de::Error>(self) -> Result<U256, E> {
        match self {
            NumberOrHexU256::Int(num) => {
                U256::from_str(num.to_string().as_str()).map_err(E::custom)
            }
            NumberOrHexU256::Hex(val) => Ok(val),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_u256_int_or_hex() {
        #[derive(Debug, Deserialize, PartialEq, Eq)]
        struct V(#[serde(deserialize_with = "from_int_or_hex")] U256);

        proptest::proptest!(|(value: u64)| {
            let u256_val = U256::from(value);

            let num_obj = serde_json::to_string(&value).unwrap();
            let hex_obj = serde_json::to_string(&u256_val).unwrap();

            let int_val:V = serde_json::from_str(&num_obj).unwrap();
            let hex_val =  serde_json::from_str(&hex_obj).unwrap();
            assert_eq!(int_val, hex_val);
        });
    }

    #[test]
    fn test_u256_int_or_hex_opt() {
        #[derive(Debug, Deserialize, PartialEq, Eq)]
        struct V(#[serde(deserialize_with = "from_int_or_hex_opt")] Option<U256>);

        let null = serde_json::to_string(&None::<U256>).unwrap();
        let val: V = serde_json::from_str(&null).unwrap();
        assert!(val.0.is_none());

        proptest::proptest!(|(value: u64)| {
            let u256_val = U256::from(value);

            let num_obj = serde_json::to_string(&value).unwrap();
            let hex_obj = serde_json::to_string(&u256_val).unwrap();

            let int_val:V = serde_json::from_str(&num_obj).unwrap();
            let hex_val =  serde_json::from_str(&hex_obj).unwrap();
            assert_eq!(int_val, hex_val);
            assert_eq!(int_val.0, Some(u256_val));
        });
    }

    #[test]
    fn serde_hex_or_number_u64() {
        #[derive(Debug, Deserialize, PartialEq, Eq)]
        struct V(U64HexOrNumber);

        proptest::proptest!(|(value: u64)| {
            let val = U64::from(value);

            let num_obj = serde_json::to_string(&value).unwrap();
            let hex_obj = serde_json::to_string(&val).unwrap();

            let int_val:V = serde_json::from_str(&num_obj).unwrap();
            let hex_val =  serde_json::from_str(&hex_obj).unwrap();
            assert_eq!(int_val, hex_val);
        });
    }
}
