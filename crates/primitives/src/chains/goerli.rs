use ethers_core::types::Chain;

use crate::{ForkId, Hardfork, H256};

use super::specs::{Builtin, NetworkUpgrades};

#[derive(Debug, Default, Clone)]
pub struct GoerliSpec;

impl Builtin for GoerliSpec {
    const ID: Chain = Chain::Goerli;
    const GENESIS_HASH: H256 = todo!();
}

impl NetworkUpgrades for GoerliSpec {
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
