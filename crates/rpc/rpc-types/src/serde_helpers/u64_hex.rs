//! Helper to deserialize an `u64` from [U64] accepting a hex quantity string with optional 0x
//! prefix

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
