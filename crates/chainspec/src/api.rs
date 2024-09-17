use core::fmt::Debug;

use alloy_chains::Chain;

use crate::ChainSpec;

/// Trait representing type configuring a chain spec.
pub trait EthChainSpec: Send + Sync + Unpin + Debug + 'static {
    /// Hardfork type of network stack.
    type Hardfork;

    /// Chain id.
    fn chain(&self) -> Chain;
}

impl<T> EthChainSpec for ChainSpec<T>
where
    T: Send + Sync + Unpin + Debug + 'static,
{
    type Hardfork = T;

    fn chain(&self) -> Chain {
        self.chain
    }
}
