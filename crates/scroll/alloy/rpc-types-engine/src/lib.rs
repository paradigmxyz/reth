//! Scroll types for interaction with the Engine API via RPC.

#![cfg_attr(not(feature = "std"), no_std)]

mod attributes;
pub use attributes::ScrollPayloadAttributes;

extern crate alloc;
