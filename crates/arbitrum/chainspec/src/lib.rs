#![cfg_attr(not(feature = "std"), no_std)]

#[derive(Clone, Debug, Default)]
pub struct ArbChainSpec {
    pub chain_id: u64,
}

impl ArbChainSpec {
    pub const fn chain_id(&self) -> u64 {
        self.chain_id
    }
}
