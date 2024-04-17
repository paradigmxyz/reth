/// The `compat` module contains utility functions that perform conversions between reth and revm,
/// compare analogous types from the two implementations, and calculate intrinsic gas usage.
///
/// The included conversion methods can be used to convert between:
/// * reth's [Log](crate::Log) type and revm's [Log](revm_primitives::Log) type.
/// * reth's [Account](crate::Account) type and revm's [AccountInfo](revm_primitives::AccountInfo)
///   type.
pub mod compat;

/// Reth block execution/validation configuration and constants
pub mod config;

/// The `env` module provides utility methods for filling revm transaction and block environments.
///
/// It includes functions to fill transaction and block environments with relevant data, prepare
/// the block and transaction environments for system contract calls, and recover the signer from
/// Clique-formatted extra data in ethereum headers.
pub mod env;
