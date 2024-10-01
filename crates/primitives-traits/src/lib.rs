//! Common abstracted types in Reth.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

#[macro_use]
extern crate alloc;

/// Common constants.
pub mod constants;
pub use constants::gas_units::{format_gas, format_gas_throughput};

/// Minimal account
pub mod account;
pub use account::{Account, Bytecode};

mod integer_list;
pub use integer_list::{IntegerList, IntegerListError};

pub mod request;
pub use request::{Request, Requests};

mod withdrawal;
pub use withdrawal::{Withdrawal, Withdrawals};

mod error;
pub use error::{GotExpected, GotExpectedBoxed};

mod log;
pub use log::{logs_bloom, Log, LogData};

mod storage;
pub use storage::StorageEntry;

/// Common header types
pub mod header;
#[cfg(any(test, feature = "arbitrary", feature = "test-utils"))]
pub use header::test_utils;
pub use header::{BlockHeader, Header, HeaderError, SealedHeader};

/// Bincode-compatible serde implementations for common abstracted types in Reth.
///
/// `bincode` crate doesn't work with optionally serializable serde fields, but some of the
/// Reth types require optional serialization for RPC compatibility. This module makes so that
/// all fields are serialized.
///
/// Read more: <https://github.com/bincode-org/bincode/issues/326>
#[cfg(feature = "serde-bincode-compat")]
pub mod serde_bincode_compat {
    pub use super::header::{serde_bincode_compat as header, serde_bincode_compat::*};
}
