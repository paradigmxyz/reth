use reth_chainspec::ChainSpec;
use std::sync::Arc;

/// A trait for reading the current chainspec.
#[auto_impl::auto_impl(&, Arc)]
pub trait ChainSpecProvider: Send + Sync {
    /// Get an [`Arc`] to the chainspec.
    fn chain_spec(&self) -> Arc<ChainSpec>;
}
