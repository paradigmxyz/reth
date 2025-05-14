//! Engine Payload related types.

#![cfg_attr(not(feature = "std"), no_std)]
#[cfg(not(feature = "std"))]
extern crate alloc as std;

pub use base_fee::{
    PayloadBuildingBaseFeeProvider, L1_BASE_FEE_OVERHEAD, L1_BASE_FEE_PRECISION,
    L1_BASE_FEE_SCALAR, L1_BASE_FEE_SLOT, MAX_L2_BASE_FEE,
};
mod base_fee;

pub mod builder;
pub use builder::{ScrollPayloadBuilder, ScrollPayloadTransactions};

pub mod config;
pub use config::ScrollBuilderConfig;

mod error;
pub use error::ScrollPayloadBuilderError;

#[cfg(feature = "test-utils")]
mod test_utils;

#[cfg(feature = "test-utils")]
pub use test_utils::{NoopPayloadJob, NoopPayloadJobGenerator};
