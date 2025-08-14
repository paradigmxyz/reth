#![cfg_attr(not(feature = "std"), no_std)]

pub mod engine;
pub mod nitro;
pub mod eth;
pub mod error;

pub struct ArbRpc;

pub use error::ArbEthApiError;

#[cfg(feature = "std")]
pub use nitro::{ArbNitroApiServer, ArbNitroRpc};

#[cfg(feature = "std")]
pub use eth::ArbEthApiBuilder;
