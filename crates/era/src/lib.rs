//! Era and Era1 files support for Ethereum history expiry.
//!
//! Era1 files use the same e2store foundation but are specialized for
//! execution layer block history, following the format:
//! Version | block-tuple* | other-entries* | Accumulator | `BlockIndex`
//!
//! Era files are special instances of `.e2s` files with a strict content format
//! optimized for reading and long-term storage and distribution.
//!
//! See also:
//! - E2store format: <https://github.com/status-im/nimbus-eth2/blob/stable/docs/e2store.md>
//! - Era format: <https://github.com/eth-clients/e2store-format-specs/blob/main/formats/era.md>
//! - Era1 format: <https://github.com/eth-clients/e2store-format-specs/blob/main/formats/era1.md>

pub mod common;
pub mod consensus_types;
pub mod e2s;
pub mod era1;
pub mod era_types;

#[cfg(test)]
pub(crate) mod test_utils;
