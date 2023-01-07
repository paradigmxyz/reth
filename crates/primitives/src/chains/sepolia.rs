use ethers_core::types::Chain;

use crate::{ForkId, Hardfork, H256};

use super::specs::{Builtin, Hardforks};

#[derive(Debug, Default, Clone)]
pub struct SepoliaSpec;

impl Builtin for SepoliaSpec {
    const ID: Chain = Chain::Sepolia;
    const GENESIS_HASH: H256 = todo!();
}

impl Hardforks for SepoliaSpec {
    fn fork_block(&self, fork: &Hardfork) -> u64 {
        todo!()
    }

    fn fork_id(&self, fork: &Hardfork) -> ForkId {
        todo!()
    }
}
