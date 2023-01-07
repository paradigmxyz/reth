use crate::{ForkId, Hardfork, H256};

pub use specs::{ChainSpec, Essentials, Hardforks};

mod goerli;
mod mainnet;
mod sepolia;
mod specs;

pub use goerli::GoerliSpec;
pub use mainnet::MainnetSpec;
pub use sepolia::SepoliaSpec;

#[derive(Debug, Clone)]
pub enum ChainSpecUnified {
    Mainnet,
    Goerli,
    Sepolia,
    // TODO: Add Custom(CustomChainSpec) variant
}

impl Essentials for ChainSpecUnified {
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
}

impl Hardforks for ChainSpecUnified {
    fn fork_block(&self, fork: &Hardfork) -> u64 {
        todo!()
    }

    fn fork_id(&self, fork: &Hardfork) -> ForkId {
        todo!()
    }
}
