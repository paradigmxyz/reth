use reth_primitives::ChainSpec;
use std::sync::Arc;

/// A trait for reading the current chainspec.
pub trait ChainSpecProvider: Send + Sync {
    /// Get an [`Arc`] to the chainspec.
    fn chain_spec(&self) -> Arc<ChainSpec>;
}
