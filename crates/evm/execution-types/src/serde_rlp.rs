//! Serde adapter preserving human-readable values and using RLP byte strings in binary formats.
//!
//! Use with `#[serde(with = "reth_execution_types::serde_rlp")]` on fields implementing
//! both Serde and RLP serialization. Binary decoding requires exactly one RLP value and rejects
//! trailing bytes.
//!
//! ```
//! use alloy_consensus::Header;
//! use serde::{Deserialize, Serialize};
//!
//! #[derive(Serialize, Deserialize)]
//! struct Proof {
//!     #[serde(with = "reth_execution_types::serde_rlp")]
//!     header: Header,
//! }
//! ```

use alloy_rlp::{Decodable, Encodable};
use core::{fmt, marker::PhantomData};
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

/// Serialize a value normally in human-readable formats, or as RLP bytes otherwise.
pub fn serialize<T: Serialize + Encodable, S: Serializer>(
    value: &T,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    if serializer.is_human_readable() {
        value.serialize(serializer)
    } else {
        serializer.serialize_bytes(&alloy_rlp::encode(value))
    }
}

/// Deserialize a structured human-readable value or an exact RLP byte string.
pub fn deserialize<'de, T: Deserialize<'de> + Decodable, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<T, D::Error> {
    if deserializer.is_human_readable() {
        T::deserialize(deserializer)
    } else {
        deserializer.deserialize_bytes(RlpVisitor(PhantomData))
    }
}

struct RlpVisitor<T>(PhantomData<T>);

impl<T: Decodable> de::Visitor<'_> for RlpVisitor<T> {
    type Value = T;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an RLP-encoded byte string")
    }

    fn visit_bytes<E: de::Error>(self, encoded: &[u8]) -> Result<T, E> {
        alloy_rlp::decode_exact(encoded).map_err(E::custom)
    }
}
