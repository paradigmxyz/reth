#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod receipts;
pub use receipts::*;
pub struct ArbEvmConfig;

impl Default for ArbEvmConfig {
    fn default() -> Self {
        Self
    }
}
