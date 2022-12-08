//! Reth block execution/validation configuration and constants

use reth_primitives::{BlockNumber, U256};

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
    pub fn new_test_homestead() -> Self {
        Self { homestead: 0, ..Self::new_ethereum() }
    }

    /// New tangerine enabled spec
    pub fn new_test_tangerine_whistle() -> Self {
        Self { tangerine_whistle: 0, ..Self::new_test_homestead() }
    }

    /// New spurious_dragon enabled spec
    pub fn new_test_spurious_dragon() -> Self {
        Self { spurious_dragon: 0, ..Self::new_test_tangerine_whistle() }
    }

    /// New byzantium enabled spec
    pub fn new_test_byzantium() -> Self {
        Self { byzantium: 0, ..Self::new_test_spurious_dragon() }
    }

    /// New petersburg enabled spec
    pub fn new_test_petersburg() -> Self {
        Self { petersburg: 0, ..Self::new_test_byzantium() }
    }

    /// New istanbul enabled spec
    pub fn new_test_istanbul() -> Self {
        Self { istanbul: 0, ..Self::new_test_petersburg() }
    }

    /// New berlin enabled spec
    pub fn new_test_berlin() -> Self {
        Self { berlin: 0, ..Self::new_test_istanbul() }
    }

    /// New london enabled spec
    pub fn new_test_london() -> Self {
        Self { london: 0, ..Self::new_test_berlin() }
    }

    /// New paris enabled spec
    pub fn new_test_paris() -> Self {
        Self { paris: 0, ..Self::new_test_london() }
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
