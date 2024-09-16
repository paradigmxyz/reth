use core::fmt::Debug;

use alloy_chains::Chain;

use crate::ChainSpec;

/// Trait representing type configuring a chain spec.
pub trait EthChainSpec: Send + Sync + Unpin + Debug + 'static {
    // todo: make chain spec type generic over hardfork
    //type Hardfork: Clone + Copy + 'static;

    /// Chain id.
    fn chain(&self) -> Chain;
}

impl<T> EthChainSpec for ChainSpec<T>
where
    T: Send + Sync + Unpin + Debug + 'static,
{
    fn chain(&self) -> Chain {
        self.chain
    }
}
