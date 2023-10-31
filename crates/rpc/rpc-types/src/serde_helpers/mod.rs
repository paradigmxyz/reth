//! Serde helpers for primitive types.

use alloy_primitives::U256;
use serde::{Deserialize, Deserializer, Serializer};

pub mod json_u256;
pub mod num;
/// Storage related helpers.
pub mod storage;
pub mod u64_hex;

/// Deserializes the input into a U256, accepting both 0x-prefixed hex and decimal strings with
/// arbitrary precision, defined by serde_json's [`Number`](serde_json::Number).
pub fn from_int_or_hex<'de, D>(deserializer: D) -> Result<U256, D::Error>
where
    D: Deserializer<'de>,
{
    num::NumberOrHexU256::deserialize(deserializer)?.try_into_u256()
}

/// Serialize a byte vec as a hex string _without_ the "0x" prefix.
///
/// This behaves the same as [`hex::encode`](alloy_primitives::hex::encode).
pub fn serialize_hex_string_no_prefix<S, T>(x: T, s: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
    T: AsRef<[u8]>,
{
    s.serialize_str(&alloy_primitives::hex::encode(x.as_ref()))
}
