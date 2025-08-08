#![cfg_attr(not(feature = "std"), no_std)]

pub trait ArbitrumChainSpec {
    fn chain_id(&self) -> u64;
}

#[derive(Clone, Debug, Default)]
pub struct ArbChainSpec {
    pub chain_id: u64,
}

impl ArbitrumChainSpec for ArbChainSpec {
    fn chain_id(&self) -> u64 {
        self.chain_id
    }
}
