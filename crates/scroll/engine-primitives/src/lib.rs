//! The engine primitives for Scroll.

#![cfg_attr(not(feature = "std"), no_std)]
#[cfg(not(feature = "std"))]
extern crate alloc as std;

mod payload;
pub use payload::{
    try_into_block, ScrollBuiltPayload, ScrollEngineTypes, ScrollPayloadBuilderAttributes,
    ScrollPayloadTypes,
};

extern crate alloc;
