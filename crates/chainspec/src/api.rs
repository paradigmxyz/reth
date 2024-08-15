use crate::ChainSpec;
use alloy_chains::Chain;
use reth_ethereum_forks::{EthereumHardfork, ForkCondition};

/// Trait representing type configuring a chain spec.
pub trait ChainSpecTrait: Send + Sync + Unpin + 'static {
    /// Enumw with chain hardforks.
    type Hardfork: Clone + Copy + 'static;

    /// Chain id.
    fn chain(&self) -> Chain;
    /// Activation condition for a given hardfork.
    fn activation_condition(&self, hardfork: Self::Hardfork) -> ForkCondition;
}

impl ChainSpecTrait for ChainSpec {
    type Hardfork = EthereumHardfork;

    fn chain(&self) -> Chain {
        self.chain
    }

    fn activation_condition(&self, hardfork: Self::Hardfork) -> ForkCondition {
        self.hardforks.fork(hardfork)
    }
}
