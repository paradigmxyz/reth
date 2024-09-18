use crate::ChainSpec;
use alloy_chains::Chain;
use core::fmt::Debug;

/// Trait representing type configuring a chain spec.
pub trait EthChainSpec: Send + Sync + Unpin + Debug + 'static {
    // todo: make chain spec type generic over hardfork
    //type Hardfork: Clone + Copy + 'static;

    /// Chain id.
    fn chain(&self) -> Chain;
}

impl EthChainSpec for ChainSpec {
    fn chain(&self) -> Chain {
        self.chain
    }
}
