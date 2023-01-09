use super::{ChainSpec, ChainSpecUnified, Essentials, NetworkUpgrades};

#[derive(Debug, Clone, Copy)]
pub struct CustomChainSpec;

impl Essentials for CustomChainSpec {
    fn id(&self) -> u64 {
        todo!()
    }

    fn genesis_hash(&self) -> revm_interpreter::B256 {
        todo!()
    }
}

impl NetworkUpgrades for CustomChainSpec {
    fn fork_block(&self, fork: crate::Hardfork) -> u64 {
        todo!()
    }

    fn fork_id(&self, fork: crate::Hardfork) -> crate::ForkId {
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

pub struct CustomChainSpecBuilder;

impl CustomChainSpecBuilder {
    pub fn from<CS: ChainSpec>(spec: CS) -> Self {
        todo!()
    }

    pub fn paris_activated(&self) -> Self {
        todo!()
    }

    pub fn london_activated(&self) -> Self {
        todo!()
    }

    pub fn berlin_activated(&self) -> Self {
        todo!()
    }

    pub fn istanbul_activated(&self) -> Self {
        todo!()
    }

    pub fn petersburg_activated(&self) -> Self {
        todo!()
    }

    pub fn byzantium_activated(&self) -> Self {
        todo!()
    }

    pub fn spurious_dragon_activated(&self) -> Self {
        todo!()
    }

    pub fn tangerine_whistle_activated(&self) -> Self {
        todo!()
    }

    pub fn homestead_activated(&self) -> Self {
        todo!()
    }

    pub fn frontier_activated(&self) -> Self {
        todo!()
    }

    pub fn build(&self) -> ChainSpecUnified {
        ChainSpecUnified::Other(CustomChainSpec)
    }
}
