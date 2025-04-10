//! Era and Era1 files support for Ethereum history expiry.
//!
//!
//! Era files are special instances of .e2s files with a strict content format
//! optimized for reading and long-term storage and distribution.
//!
//! Era1 files use the same e2store foundation but are specialized for
//! execution layer block history, following the format:
//! Version | block-tuple* | other-entries* | Accumulator | `BlockIndex`
//!
//! See also:
//! - E2store format: <https://github.com/status-im/nimbus-eth2/blob/stable/docs/e2store.md>
//! - Era1 format: <https://github.com/eth-clients/e2store-format-specs/blob/main/formats/era1.md>

pub mod e2s_file;
pub mod e2s_types;
pub mod era1_file;
pub mod era1_types;
pub mod execution_types;
