use crate::{BlockNumber, ForkFilter, ForkId, Hardfork, H256};
use core::fmt::Debug;
use ethers_core::types::Chain;

pub trait ChainSpec: Essentials + Hardforks {}

pub trait Essentials: 'static + Debug + Sync + Send + Unpin {
    fn id(&self) -> u64;
    fn genesis_hash(&self) -> H256;
}

pub trait Hardforks: Essentials {
    fn fork_block(&self, fork: &Hardfork) -> u64;
    fn fork_id(&self, fork: &Hardfork) -> ForkId;

    /// Creates a [`ForkFilter`](crate::ForkFilter) for the given hardfork.
    ///
    /// **CAUTION**: This assumes the current hardfork's block number is the current head and uses
    /// all known future hardforks to initialize the filter.
    fn fork_filter(&self, fork: Hardfork) -> ForkFilter {
        let all_forks = Hardfork::all_forks();
        let future_forks: Vec<BlockNumber> = all_forks
            .iter()
            .filter(|f| self.fork_block(f) > self.fork_block(&fork))
            .map(|f| self.fork_block(f))
            .collect();

        // this data structure is not chain-agnostic, so we can pass in the constant mainnet
        // genesis
        ForkFilter::new(self.fork_block(&fork), self.genesis_hash(), future_forks)
    }
}

pub trait Builtin: 'static + Debug + Sync + Send + Unpin {
    const ID: Chain;
    const GENESIS_HASH: H256;
}

impl<T: Essentials + Hardforks> ChainSpec for T {}

impl<T: Builtin> Essentials for T {
    fn id(&self) -> u64 {
        Self::ID as u64
    }

    fn genesis_hash(&self) -> H256 {
        Self::GENESIS_HASH
    }
}
