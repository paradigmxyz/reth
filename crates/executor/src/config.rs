//! Reth block execution/validation configuration and constants

use reth_primitives::{BlockNumber, U256};

/// Two ethereum worth of wei
pub const WEI_2ETH: u128 = 2000000000000000000u128;
/// Three ethereum worth of wei
pub const WEI_3ETH: u128 = 3000000000000000000u128;
/// Five ethereum worth of wei
pub const WEI_5ETH: u128 = 5000000000000000000u128;

/// Configuration for executor
#[derive(Debug, Clone)]
pub struct Config {
    /// Chain id.
    pub chain_id: U256,
    /// Spec upgrades.
    pub spec_upgrades: SpecUpgrades,
}

impl Config {
    /// Create new config for ethereum.
    pub fn new_ethereum() -> Self {
        Self { chain_id: 1.into(), spec_upgrades: SpecUpgrades::new_ethereum() }
    }
}

/// Spec with there ethereum codenames.
#[derive(Debug, Clone)]
#[allow(missing_docs)]
pub struct SpecUpgrades {
    pub frontier: BlockNumber,
    //pub frontier_thawing: BlockNumber,
    pub homestead: BlockNumber,
    //pub dao_fork: BlockNumber,
    pub tangerine_whistle: BlockNumber,
    pub spurious_dragon: BlockNumber,
    pub byzantium: BlockNumber,
    //pub constantinople: BlockNumber,
    pub petersburg: BlockNumber, //Overrider Constantinople
    pub istanbul: BlockNumber,
    //pub muir_glacier: BlockNumber,
    pub berlin: BlockNumber,
    pub london: BlockNumber,
    //pub arrow_glacier: BlockNumber,
    //pub gray_glacier: BlockNumber,
    pub paris: BlockNumber, // Aka the merge
    pub shanghai: BlockNumber,
}

impl SpecUpgrades {
    /// After merge/peric block reward was removed from execution layer.
    pub fn has_block_reward(&self, block_num: BlockNumber) -> bool {
        block_num <= self.paris
    }

    /// Ethereum mainnet spec
    pub fn new_ethereum() -> Self {
        Self {
            frontier: 0,
            //frontier_thawing: 200000,
            homestead: 1150000,
            //dao_fork: 1920000,
            tangerine_whistle: 2463000,
            spurious_dragon: 2675000,
            byzantium: 4370000,
            //constantinople: 7280000,
            petersburg: 7280000, //Overrider Constantinople
            istanbul: 9069000,
            //muir_glacier: 9200000,
            berlin: 12244000,
            london: 12965000,
            //arrow_glacier: 13773000,
            //gray_glacier: 15050000,
            paris: 15537394, // TheMerge,
            shanghai: u64::MAX,
        }
    }

    /// New homestead enabled spec
    pub fn new_homestead_activated() -> Self {
        Self { homestead: 0, ..Self::new_ethereum() }
    }

    /// New tangerine enabled spec
    pub fn new_tangerine_whistle_activated() -> Self {
        Self { tangerine_whistle: 0, ..Self::new_homestead_activated() }
    }

    /// New spurious_dragon enabled spec
    pub fn new_spurious_dragon_activated() -> Self {
        Self { spurious_dragon: 0, ..Self::new_tangerine_whistle_activated() }
    }

    /// New byzantium enabled spec
    pub fn new_byzantium_activated() -> Self {
        Self { byzantium: 0, ..Self::new_spurious_dragon_activated() }
    }

    /// New petersburg enabled spec
    pub fn new_petersburg_activated() -> Self {
        Self { petersburg: 0, ..Self::new_byzantium_activated() }
    }

    /// New istanbul enabled spec
    pub fn new_istanbul_activated() -> Self {
        Self { istanbul: 0, ..Self::new_petersburg_activated() }
    }

    /// New berlin enabled spec
    pub fn new_berlin_activated() -> Self {
        Self { berlin: 0, ..Self::new_istanbul_activated() }
    }

    /// New london enabled spec
    pub fn new_london_activated() -> Self {
        Self { london: 0, ..Self::new_berlin_activated() }
    }

    /// New paris enabled spec
    pub fn new_paris_activated() -> Self {
        Self { paris: 0, ..Self::new_london_activated() }
    }

    /// return revm_spec from spec configuration.
    pub fn revm_spec(&self, for_block: BlockNumber) -> revm::SpecId {
        match for_block {
            b if self.shanghai >= b => revm::MERGE_EOF,
            b if self.paris >= b => revm::MERGE,
            b if self.london >= b => revm::LONDON,
            b if self.berlin >= b => revm::BERLIN,
            b if self.istanbul >= b => revm::ISTANBUL,
            b if self.petersburg >= b => revm::PETERSBURG,
            b if self.byzantium >= b => revm::BYZANTIUM,
            b if self.spurious_dragon >= b => revm::SPURIOUS_DRAGON,
            b if self.tangerine_whistle >= b => revm::TANGERINE,
            b if self.homestead >= b => revm::HOMESTEAD,
            b if self.frontier >= b => revm::FRONTIER,
            _ => panic!("wrong configuration"),
        }
    }
}
