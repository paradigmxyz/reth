use ethers_core::types::Chain;
use hex_literal::hex;
use std::fmt::Debug;

use crate::{hardfork::Hardfork, BlockNumber, ForkFilter, H256, ForkId};

pub trait ChainSpec: 'static + Debug + Sync + Send + Unpin {
    fn id(&self) -> u64;
    fn genesis_hash(&self) -> H256;

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

#[derive(Debug, Default, Clone)]
pub struct MainnetSpec;

impl ChainSpec for MainnetSpec {
    fn id(&self) -> u64 {
        Chain::Mainnet as u64
    }

    fn genesis_hash(&self) -> H256 {
        H256(hex!("d4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3"))
    }

    fn fork_block(&self, fork: &Hardfork) -> u64 {
        todo!()
    }

    fn fork_id(&self, fork: &Hardfork) -> ForkId {
        todo!()
    }
}

#[derive(Debug, Default, Clone)]
pub struct GoerliSpec;

impl ChainSpec for GoerliSpec {
    fn id(&self) -> u64 {
        todo!()
    }

    fn genesis_hash(&self) -> H256 {
        todo!()
    }

    fn fork_block(&self, fork: &Hardfork) -> u64 {
        todo!()
    }

    fn fork_id(&self, fork: &Hardfork) -> ForkId {
        todo!()
    }
}

#[derive(Debug, Default, Clone)]
pub struct SepoliaSpec;

impl ChainSpec for SepoliaSpec {
    fn id(&self) -> u64 {
        todo!()
    }

    fn genesis_hash(&self) -> H256 {
        todo!()
    }

    fn fork_block(&self, fork: &Hardfork) -> u64 {
        todo!()
    }

    fn fork_id(&self, fork: &Hardfork) -> ForkId {
        todo!()
    }
}

#[derive(Debug, Clone)]
pub enum ChainSpecUnified {
    Mainnet,
    Goerli,
    Sepolia,
    // TODO: Add Custom(CustomChainSpec) variant
}

impl ChainSpec for ChainSpecUnified {
    fn id(&self) -> u64 {
        match &self {
            ChainSpecUnified::Mainnet => MainnetSpec::default().id(),
            ChainSpecUnified::Goerli => GoerliSpec::default().id(),
            ChainSpecUnified::Sepolia => SepoliaSpec::default().id(),
        }
    }

    fn genesis_hash(&self) -> H256 {
        todo!()
    }

    fn fork_block(&self, fork: &Hardfork) -> u64 {
        todo!()
    }

    fn fork_id(&self, fork: &Hardfork) -> ForkId {
        todo!()
    }
}
