use reth_primitives::ChainSpec;
use std::sync::Arc;

/// A trait for reading the current chainspec.
pub trait ChainSpecReader: Send + Sync {
    /// Get an [`Arc`] to the chainspec.
    fn spec(&self) -> Arc<ChainSpec>;
}
