//! Helpers for working with revm.

/// The `env` module provides utility methods for filling revm transaction and block environments.
///
/// It includes functions to fill transaction and block environments with relevant data, prepare
/// the block and transaction environments for system contract calls, and recover the signer from
/// Clique-formatted extra data in ethereum headers.
pub mod env;
