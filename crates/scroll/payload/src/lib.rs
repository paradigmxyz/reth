//! Engine Payload related types.

#![cfg_attr(feature = "optimism", allow(unused_crate_dependencies))]
// The `scroll` feature must be enabled to use this crate.
#![cfg(all(feature = "scroll", not(feature = "optimism")))]
#![cfg_attr(not(feature = "std"), no_std)]

mod builder;
pub use builder::{ScrollEmptyPayloadBuilder, ScrollPayloadTransactions};

#[cfg(feature = "test-utils")]
mod test_utils;
#[cfg(feature = "test-utils")]
pub use test_utils::{NoopPayloadJob, NoopPayloadJobGenerator};
