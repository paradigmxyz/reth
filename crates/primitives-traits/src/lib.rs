//! Common abstracted types in reth.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
// TODO: remove when https://github.com/proptest-rs/proptest/pull/427 is merged
#![allow(unknown_lints, non_local_definitions)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "alloy-compat")]
mod alloy_compat;

/// Common constants.
pub mod constants;

/// Minimal account
pub mod account;
pub use account::{Account, Bytecode};

mod integer_list;
pub use integer_list::IntegerList;

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
pub use header::{Header, HeaderError, SealedHeader};
