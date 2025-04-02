//! Engine Payload related types.

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(not(feature = "std"))]
extern crate alloc as std;

pub mod builder;
pub use builder::{ScrollPayloadBuilder, ScrollPayloadTransactions};

mod error;
pub use error::ScrollPayloadBuilderError;

#[cfg(feature = "test-utils")]
mod test_utils;
#[cfg(feature = "test-utils")]
pub use test_utils::{NoopPayloadJob, NoopPayloadJobGenerator};
