use alloy_chains::Chain;
use reth_ethereum_forks::{EthereumHardfork, ForkCondition};

use crate::ChainSpec;

pub trait ChainSpecTrait: Send + Sync + Unpin + 'static {
    type Hardfork: Clone + Copy + 'static;

    fn chain(&self) -> Chain;
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
