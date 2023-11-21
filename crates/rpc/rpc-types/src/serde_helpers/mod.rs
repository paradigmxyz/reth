//! Serde helpers for primitive types.

use alloy_primitives::B256;
use serde::Serializer;

pub mod json_u256;
pub use json_u256::JsonU256;

/// Helpers for dealing with numbers.
pub mod num;
pub use num::*;

/// Storage related helpers.
pub mod storage;
pub use storage::JsonStorageKey;

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

/// Serialize a [B256] as a hex string _without_ the "0x" prefix.
pub fn serialize_b256_hex_string_no_prefix<S>(x: &B256, s: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    s.serialize_str(&format!("{x:x}"))
}
