//! Numeric helpers

use crate::U256;
use serde::{de, Deserialize, Deserializer};
use std::str::FromStr;

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
}
