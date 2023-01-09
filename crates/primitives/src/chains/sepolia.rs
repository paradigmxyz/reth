use ethers_core::types::Chain;

use crate::{ForkId, Hardfork, H256};

use super::specs::{Builtin, NetworkUpgrades};

#[derive(Debug, Default, Clone)]
pub struct SepoliaSpec;

impl Builtin for SepoliaSpec {
    const ID: Chain = Chain::Sepolia;
    const GENESIS_HASH: H256 = todo!();
}

impl NetworkUpgrades for SepoliaSpec {
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
