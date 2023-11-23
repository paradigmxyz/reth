//! Numeric serde helpers.

use alloy_primitives::{U256, U64};
use serde::{de, Deserialize, Deserializer, Serialize};
use std::str::FromStr;

/// A `u64` wrapper type that deserializes from hex or a u64 and serializes as hex.
///
///
/// ```rust
/// use reth_rpc_types::num::U64HexOrNumber;
/// let number_json = "100";
/// let hex_json = "\"0x64\"";
///
/// let number: U64HexOrNumber = serde_json::from_str(number_json).unwrap();
/// let hex: U64HexOrNumber = serde_json::from_str(hex_json).unwrap();
/// assert_eq!(number, hex);
/// assert_eq!(hex.to(), 100);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct U64HexOrNumber(U64);

impl U64HexOrNumber {
    /// Returns the wrapped u64
    pub fn to(self) -> u64 {
        self.0.to()
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
        value.to()
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

/// serde functions for handling `u64` as [U64]
pub mod u64_hex {
    use alloy_primitives::U64;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    /// Deserializes an `u64` from [U64] accepting a hex quantity string with optional 0x prefix
    pub fn deserialize<'de, D>(deserializer: D) -> Result<u64, D::Error>
    where
        D: Deserializer<'de>,
    {
        U64::deserialize(deserializer).map(|val| val.to())
    }

    /// Serializes u64 as hex string
    pub fn serialize<S: Serializer>(value: &u64, s: S) -> Result<S::Ok, S::Error> {
        U64::from(*value).serialize(s)
    }
}

/// serde functions for handling `Option<u64>` as [U64]
pub mod u64_hex_opt {
    use alloy_primitives::U64;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    /// Serializes u64 as hex string
    pub fn serialize<S: Serializer>(value: &Option<u64>, s: S) -> Result<S::Ok, S::Error> {
        match value {
            Some(val) => U64::from(*val).serialize(s),
            None => s.serialize_none(),
        }
    }

    /// Deserializes an `Option` from [U64] accepting a hex quantity string with optional 0x prefix
    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(U64::deserialize(deserializer)
            .map_or(None, |v| Some(u64::from_be_bytes(v.to_be_bytes()))))
    }
}

/// serde functions for handling primitive `u64` as [U64]
pub mod u64_hex_or_decimal {
    use crate::serde_helpers::num::U64HexOrNumber;
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

/// serde functions for handling primitive optional `u64` as [U64]
pub mod u64_hex_or_decimal_opt {
    use crate::serde_helpers::num::U64HexOrNumber;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    /// Deserializes an `u64` accepting a hex quantity string with optional 0x prefix or
    /// a number
    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
    where
        D: Deserializer<'de>,
    {
        match Option::<U64HexOrNumber>::deserialize(deserializer)? {
            Some(val) => Ok(Some(val.into())),
            None => Ok(None),
        }
    }

    /// Serializes u64 as hex string
    pub fn serialize<S: Serializer>(value: &Option<u64>, s: S) -> Result<S::Ok, S::Error> {
        match value {
            Some(val) => U64HexOrNumber::from(*val).serialize(s),
            None => s.serialize_none(),
        }
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

/// An enum that represents either a [serde_json::Number] integer, or a hex [U256].
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum NumberOrHexU256 {
    /// An integer
    Int(serde_json::Number),
    /// A hex U256
    Hex(U256),
}

impl NumberOrHexU256 {
    /// Tries to convert this into a [U256]].
    pub fn try_into_u256<E: de::Error>(self) -> Result<U256, E> {
        match self {
            NumberOrHexU256::Int(num) => {
                U256::from_str(num.to_string().as_str()).map_err(E::custom)
            }
            NumberOrHexU256::Hex(val) => Ok(val),
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[test]
    fn test_hex_u64() {
        #[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
        struct Value {
            #[serde(with = "u64_hex")]
            inner: u64,
        }

        let val = Value { inner: 1000 };
        let s = serde_json::to_string(&val).unwrap();
        assert_eq!(s, "{\"inner\":\"0x3e8\"}");

        let deserialized: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(val, deserialized);
    }
}
