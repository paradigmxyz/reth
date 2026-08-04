//! Log value hashes.
//!
//! A log contributes one searchable *log value* per address and per topic. Both are reduced to a
//! 32-byte hash before being placed on a filter map; those hashes are the input to
//! [`Params::row_index`](crate::Params::row_index) and
//! [`Params::column_index`](crate::Params::column_index).

use alloy_primitives::{Address, B256};
use sha2::{Digest, Sha256};

/// Returns the log value hash of a log emitting address.
pub fn address_value(address: Address) -> B256 {
    log_value_hash(address.as_slice())
}

/// Returns the log value hash of a log topic.
pub fn topic_value(topic: B256) -> B256 {
    log_value_hash(topic.as_slice())
}

/// Addresses and topics are hashed identically; only the input width differs. Sharing the body
/// keeps the two from drifting apart, since a filter map cannot tell them back apart afterwards.
fn log_value_hash(bytes: &[u8]) -> B256 {
    B256::from(<[u8; 32]>::from(Sha256::digest(bytes)))
}
