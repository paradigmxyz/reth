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
        block_num < self.paris
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
        Self {
            homestead: 0,
            frontier: u64::MAX,
            tangerine_whistle: u64::MAX,
            spurious_dragon: u64::MAX,
            byzantium: u64::MAX,
            petersburg: u64::MAX,
            istanbul: u64::MAX,
            berlin: u64::MAX,
            london: u64::MAX,
            paris: u64::MAX,
            shanghai: u64::MAX,
        }
    }

    /// New homestead enabled spec
    pub fn new_frontier_activated() -> Self {
        Self { frontier: 0, ..Self::new_ethereum() }
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
            b if b >= self.shanghai => revm::MERGE_EOF,
            b if b >= self.paris => revm::MERGE,
            b if b >= self.london => revm::LONDON,
            b if b >= self.berlin => revm::BERLIN,
            b if b >= self.istanbul => revm::ISTANBUL,
            b if b >= self.petersburg => revm::PETERSBURG,
            b if b >= self.byzantium => revm::BYZANTIUM,
            b if b >= self.spurious_dragon => revm::SPURIOUS_DRAGON,
            b if b >= self.tangerine_whistle => revm::TANGERINE,
            b if b >= self.homestead => revm::HOMESTEAD,
            b if b >= self.frontier => revm::FRONTIER,
            _ => panic!("wrong configuration"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SpecUpgrades;
    #[test]
    fn test_to_revm_spec() {
        assert_eq!(SpecUpgrades::new_paris_activated().revm_spec(1), revm::MERGE);
        assert_eq!(SpecUpgrades::new_london_activated().revm_spec(1), revm::LONDON);
        assert_eq!(SpecUpgrades::new_berlin_activated().revm_spec(1), revm::BERLIN);
        assert_eq!(SpecUpgrades::new_istanbul_activated().revm_spec(1), revm::ISTANBUL);
        assert_eq!(SpecUpgrades::new_petersburg_activated().revm_spec(1), revm::PETERSBURG);
        assert_eq!(SpecUpgrades::new_byzantium_activated().revm_spec(1), revm::BYZANTIUM);
        assert_eq!(
            SpecUpgrades::new_spurious_dragon_activated().revm_spec(1),
            revm::SPURIOUS_DRAGON
        );
        assert_eq!(SpecUpgrades::new_tangerine_whistle_activated().revm_spec(1), revm::TANGERINE);
        assert_eq!(SpecUpgrades::new_homestead_activated().revm_spec(1), revm::HOMESTEAD);
        assert_eq!(SpecUpgrades::new_frontier_activated().revm_spec(1), revm::FRONTIER);
    }

    #[test]
    fn test_eth_spec() {
        let spec = SpecUpgrades::new_ethereum();
        assert_eq!(spec.revm_spec(15537394 + 10), revm::MERGE);
        assert_eq!(spec.revm_spec(15537394 - 10), revm::LONDON);
        assert_eq!(spec.revm_spec(12244000 + 10), revm::BERLIN);
        assert_eq!(spec.revm_spec(12244000 - 10), revm::ISTANBUL);
        assert_eq!(spec.revm_spec(7280000 + 10), revm::PETERSBURG);
        assert_eq!(spec.revm_spec(7280000 - 10), revm::BYZANTIUM);
        assert_eq!(spec.revm_spec(2675000 + 10), revm::SPURIOUS_DRAGON);
        assert_eq!(spec.revm_spec(2675000 - 10), revm::TANGERINE);
        assert_eq!(spec.revm_spec(1150000 + 10), revm::HOMESTEAD);
        assert_eq!(spec.revm_spec(1150000 - 10), revm::FRONTIER);
    }
}
