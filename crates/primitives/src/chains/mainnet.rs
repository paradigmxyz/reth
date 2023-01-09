use crate::{ForkId, Hardfork, H256};
use ethers_core::types::Chain;
use hex_literal::hex;

use super::specs::{Builtin, NetworkUpgrades};

#[derive(Debug, Default, Clone)]
pub struct MainnetSpec;

impl Builtin for MainnetSpec {
    const ID: Chain = Chain::Mainnet;
    const GENESIS_HASH: H256 =
        H256(hex!("d4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3"));
}

impl NetworkUpgrades for MainnetSpec {
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
