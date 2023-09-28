//! [serde] utilities.

use crate::{B256, U64};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

mod storage;

pub use storage::*;

mod jsonu256;
pub use jsonu256::*;

pub mod num;

mod prune;
pub use prune::deserialize_opt_prune_mode_with_min_blocks;

/// serde functions for handling primitive `u64` as [`U64`].
pub mod u64_hex {
    use super::*;

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

/// Serialize a byte vec as a hex string _without_ the "0x" prefix.
///
/// This behaves the same as [`hex::encode`](crate::hex::encode).
pub fn serialize_hex_string_no_prefix<S, T>(x: T, s: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
    T: AsRef<[u8]>,
{
    s.serialize_str(&crate::hex::encode(x.as_ref()))
}

/// Serialize a [B256] as a hex string _without_ the "0x" prefix.
pub fn serialize_b256_hex_string_no_prefix<S>(x: &B256, s: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    s.serialize_str(&format!("{x:x}"))
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
