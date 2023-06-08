//! Various serde utilities

mod storage_key;

use serde::Serializer;
pub use storage_key::*;

mod jsonu256;
use crate::H256;
pub use jsonu256::*;

pub mod num;

/// serde functions for handling primitive `u64` as [U64](crate::U64)
pub mod u64_hex {
    use crate::U64;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    /// Deserializes an `u64` from [U64] accepting a hex quantity string with optional 0x prefix
    pub fn deserialize<'de, D>(deserializer: D) -> Result<u64, D::Error>
    where
        D: Deserializer<'de>,
    {
        U64::deserialize(deserializer).map(|val| val.as_u64())
    }

    /// Serializes u64 as hex string
    pub fn serialize<S: Serializer>(value: &u64, s: S) -> Result<S::Ok, S::Error> {
        U64::from(*value).serialize(s)
    }
}

/// serde functions for handling bytes as hex strings, such as [bytes::Bytes]
pub mod hex_bytes {
    use serde::{Deserialize, Deserializer, Serializer};

    /// Serialize a byte vec as a hex string with 0x prefix
    pub fn serialize<S, T>(x: T, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
        T: AsRef<[u8]>,
    {
        s.serialize_str(&format!("0x{}", hex::encode(x.as_ref())))
    }

    /// Deserialize a hex string into a byte vec
    /// Accepts a hex string with optional 0x prefix
    pub fn deserialize<'de, T, D>(d: D) -> Result<T, D::Error>
    where
        D: Deserializer<'de>,
        T: From<Vec<u8>>,
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
}

/// Serialize a byte vec as a hex string _without_ 0x prefix.
///
/// This behaves exactly as [hex::encode]
pub fn serialize_hex_string_no_prefix<S, T>(x: T, s: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
    T: AsRef<[u8]>,
{
    s.serialize_str(&hex::encode(x.as_ref()))
}

/// Serialize a byte vec as a hex string _without_ 0x prefix
pub fn serialize_h256_hex_string_no_prefix<S>(x: &H256, s: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let val = format!("{:?}", x);
    // skip the 0x prefix
    s.serialize_str(&val.as_str()[2..])
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
