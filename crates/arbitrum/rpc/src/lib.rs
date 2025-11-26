#![cfg_attr(not(feature = "std"), no_std)]

pub mod engine;
pub mod error;
pub mod eth;
pub mod nitro;

pub struct ArbRpc;

pub use error::ArbEthApiError;

#[cfg(feature = "std")]
pub use nitro::{ArbNitroApiServer, ArbNitroRpc};

#[cfg(feature = "std")]
pub use eth::ArbEthApiBuilder;
