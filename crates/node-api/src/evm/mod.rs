//! Traits and structs for working with a configurable EVM.

/// EVM configuration trait.
pub trait EvmConfig: ConfigureEvmEnv + Clone + Send + Sync + 'static {}

/// Traits for working with a configurable EVM.
mod traits;
pub use traits::ConfigureEvmEnv;

/// Bundle state with receipts.
mod bundle_state_with_receipts;
pub use bundle_state_with_receipts::*;
