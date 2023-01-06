use crate::{BlockNumber, ForkFilter, ForkHash, ForkId, MAINNET_GENESIS};
use std::str::FromStr;

pub struct Hardfork {
    first_block: u64,
    fork_id: ForkId,
}

impl Hardfork {
    pub const fn new(first_block: u64, next_fork_block_number: u64, hash: [u8; 4]) -> Self {
        Self { first_block, fork_id: ForkId { hash: ForkHash(hash), next: next_fork_block_number } }
    }

    pub fn fork_block(&self) -> u64 {
        self.first_block
    }

    pub fn fork_id(&self) -> ForkId {
        self.fork_id
    }
}

impl From<Hardfork> for BlockNumber {
    fn from(value: Hardfork) -> Self {
        value.first_block
    }
}
