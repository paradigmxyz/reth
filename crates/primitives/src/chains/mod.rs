use crate::{ForkId, Hardfork, H256};

pub use specs::{ChainSpec, Essentials, NetworkUpgrades};

mod custom;
mod goerli;
mod mainnet;
mod sepolia;
mod specs;

pub use goerli::GoerliSpec;
pub use mainnet::MainnetSpec;
pub use sepolia::SepoliaSpec;

use self::custom::{CustomChainSpec, CustomChainSpecBuilder};

#[derive(Debug, Clone, Copy)]
pub enum ChainSpecUnified {
    Mainnet,
    Goerli,
    Sepolia,
    Other(CustomChainSpec),
}

impl ChainSpecUnified {
    pub fn into_customized(self) -> CustomChainSpecBuilder {
        CustomChainSpecBuilder::from(self)
    }
}

impl Essentials for ChainSpecUnified {
    fn id(&self) -> u64 {
        match &self {
            ChainSpecUnified::Mainnet => MainnetSpec::default().id(),
            ChainSpecUnified::Goerli => GoerliSpec::default().id(),
            ChainSpecUnified::Sepolia => SepoliaSpec::default().id(),
            ChainSpecUnified::Other(_) => todo!(),
        }
    }

    fn genesis_hash(&self) -> H256 {
        todo!()
    }
}

impl NetworkUpgrades for ChainSpecUnified {
    fn fork_block(&self, fork: Hardfork) -> u64 {
        todo!()
    }

    fn fork_id(&self, fork: Hardfork) -> ForkId {
        todo!()
    }

    fn paris_block(&self) -> u64 {
        todo!()
    }

    fn shanghai_block(&self) -> u64 {
        todo!()
    }

    fn merge_terminal_total_difficulty(&self) -> u128 {
        todo!()
    }
}
