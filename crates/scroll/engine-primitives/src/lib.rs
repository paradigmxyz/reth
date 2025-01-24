//! The engine primitives for Scroll.

#![cfg_attr(feature = "optimism", allow(unused_crate_dependencies))]
// The `scroll` feature must be enabled to use this crate.
#![cfg(all(feature = "scroll", not(feature = "optimism")))]

mod payload;
pub use payload::{
    try_into_block, ScrollBuiltPayload, ScrollEngineTypes, ScrollPayloadBuilderAttributes,
    ScrollPayloadTypes,
};

extern crate alloc;
