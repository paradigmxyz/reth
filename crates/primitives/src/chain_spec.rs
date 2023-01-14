use hex_literal::hex;
use once_cell::sync::Lazy;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{BlockNumber, ChainId, ForkFilter, ForkHash, ForkId, Genesis, Hardfork, H256, U256};

/// The Etereum mainnet spec
pub static MAINNET: Lazy<ChainSpec> = Lazy::new(|| ChainSpec {
    chain_id: 1,
    genesis: Genesis::default(),
    genesis_hash: H256(hex!("d4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3")),
    hardforks: BTreeMap::from([
        (Hardfork::Frontier, 0),
        (Hardfork::Homestead, 1150000),
        (Hardfork::Dao, 1920000),
        (Hardfork::Tangerine, 2463000),
        (Hardfork::SpuriousDragon, 2675000),
        (Hardfork::Byzantium, 4370000),
        (Hardfork::Constantinople, 7280000),
        (Hardfork::Petersburg, 7280000),
        (Hardfork::Istanbul, 9069000),
        (Hardfork::Muirglacier, 9200000),
        (Hardfork::Berlin, 12244000),
        (Hardfork::London, 12965000),
        (Hardfork::ArrowGlacier, 13773000),
        (Hardfork::GrayGlacier, 15050000),
        (Hardfork::Latest, 15050000),
    ]),
    paris_block: Some(15537394),
    paris_ttd: Some(U256::from(58750000000000000000000_u128)),
    shanghai_block: Some(u64::MAX),
});

/// The Ethereum chain spec
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ChainSpec {
    chain_id: ChainId,
    genesis: Genesis,
    genesis_hash: H256,
    hardforks: BTreeMap<Hardfork, BlockNumber>,
    paris_block: Option<u64>,
    paris_ttd: Option<U256>,
    shanghai_block: Option<u64>,
}

impl ChainSpec {
    /// Returns the chain id
    pub fn chain_id(&self) -> ChainId {
        self.chain_id
    }

    /// Returns the chain genesis hash
    pub fn genesis_hash(&self) -> H256 {
        self.genesis_hash
    }

    /// Get the first block number of the hardfork.
    pub fn fork_block(&self, fork: Hardfork) -> Option<BlockNumber> {
        self.hardforks.get(&fork).copied()
    }

    /// Returns `true` if the given fork is active on the given block
    pub fn fork_active(&self, fork: Hardfork, current_block: BlockNumber) -> bool {
        self.fork_block(fork).map(|target| target <= current_block).unwrap_or_default()
    }

    /// Get the Paris status
    pub fn paris_status(&self) -> ParisStatus {
        match self.paris_ttd {
            Some(terminal_total_difficulty) => {
                ParisStatus::Supported { terminal_total_difficulty, block: self.paris_block }
            }
            None => ParisStatus::NotSupported,
        }
    }

    /// Get an iterator of all harforks with theirs respectives block number
    pub fn forks_iter(&self) -> impl Iterator<Item = (Hardfork, BlockNumber)> + '_ {
        self.hardforks.iter().map(|(f, b)| (*f, *b))
    }

    /// Creates a [`ForkFilter`](crate::ForkFilter) for the given hardfork.
    ///
    /// **CAUTION**: This assumes the current hardfork's block number is the current head and uses
    /// all known future hardforks to initialize the filter.
    pub fn fork_filter(&self, fork: Hardfork) -> Option<ForkFilter> {
        if let Some(fork_block) = self.fork_block(fork) {
            let future_forks =
                self.forks_iter().map(|(_, b)| b).filter(|b| *b > fork_block).collect::<Vec<_>>();

            Some(ForkFilter::new(fork_block, self.genesis_hash(), future_forks))
        } else {
            None
        }
    }

    /// Compute the forkid for the given [Hardfork]
    pub fn fork_id(&self, fork: Hardfork) -> Option<ForkId> {
        if let Some(fork_block) = self.fork_block(fork) {
            let mut curr_forkhash = ForkHash::from(self.genesis_hash());
            let mut curr_block_number = 0;

            for (_, b) in self.forks_iter() {
                if fork_block >= b {
                    if b != curr_block_number {
                        curr_forkhash += b;
                        curr_block_number = b;
                    }
                } else {
                    return Some(ForkId { hash: curr_forkhash, next: b })
                }
            }
            Some(ForkId { hash: curr_forkhash, next: 0 })
        } else {
            None
        }
    }

    /// Returns a [ChainSpecBuilder] to help build custom specs
    pub fn builder() -> ChainSpecBuilder {
        ChainSpecBuilder::default()
    }
}

/// A helper to build custom chain specs
#[derive(Debug, Default)]
pub struct ChainSpecBuilder {
    chain_id: Option<ChainId>,
    genesis: Option<Genesis>,
    genesis_hash: Option<H256>,
    hardforks: BTreeMap<Hardfork, BlockNumber>,
    paris_block: Option<u64>,
    paris_ttd: Option<U256>,
    shanghai_block: Option<u64>,
}

impl ChainSpecBuilder {
    /// Returns a [ChainSpec] builder initialized with Ethereum mainnet config
    pub fn mainnet() -> Self {
        Self {
            chain_id: Some(MAINNET.chain_id),
            genesis: Some(MAINNET.genesis.clone()),
            genesis_hash: Some(MAINNET.genesis_hash),
            hardforks: MAINNET.hardforks.clone(),
            paris_block: MAINNET.paris_block,
            paris_ttd: MAINNET.paris_ttd,
            shanghai_block: MAINNET.shanghai_block,
        }
    }

    /// Sets the chain id
    pub fn chain_id(mut self, chain_id: ChainId) -> Self {
        self.chain_id = Some(chain_id);
        self
    }

    /// Sets the genesis
    pub fn genesis(mut self, genesis: Genesis) -> Self {
        self.genesis = Some(genesis);
        self
    }

    /// Enables Frontier
    pub fn frontier_activated(mut self) -> Self {
        self.hardforks.insert(Hardfork::Frontier, 0);
        self
    }

    /// Enables Homestead
    pub fn homestead_activated(mut self) -> Self {
        self = self.frontier_activated();
        self.hardforks.insert(Hardfork::Homestead, 0);
        self
    }

    /// Enables Tangerine
    pub fn tangerine_whistle_activated(mut self) -> Self {
        self = self.homestead_activated();
        self.hardforks.insert(Hardfork::Tangerine, 0);
        self
    }

    /// Enables SpuriousDragon
    pub fn spurious_dragon_activated(mut self) -> Self {
        self = self.tangerine_whistle_activated();
        self.hardforks.insert(Hardfork::SpuriousDragon, 0);
        self
    }

    /// Enables Byzantium
    pub fn byzantium_activated(mut self) -> Self {
        self = self.spurious_dragon_activated();
        self.hardforks.insert(Hardfork::Byzantium, 0);
        self
    }

    /// Enables Petersburg
    pub fn petersburg_activated(mut self) -> Self {
        self = self.byzantium_activated();
        self.hardforks.insert(Hardfork::Petersburg, 0);
        self
    }

    /// Enables Istanbul
    pub fn istanbul_activated(mut self) -> Self {
        self = self.petersburg_activated();
        self.hardforks.insert(Hardfork::Istanbul, 0);
        self
    }

    /// Enables Berlin
    pub fn berlin_activated(mut self) -> Self {
        self = self.istanbul_activated();
        self.hardforks.insert(Hardfork::Berlin, 0);
        self
    }

    /// Enables London
    pub fn london_activated(mut self) -> Self {
        self = self.berlin_activated();
        self.hardforks.insert(Hardfork::London, 0);
        self
    }

    /// Enables Paris
    pub fn paris_activated(mut self) -> Self {
        self = self.berlin_activated();
        self.paris_block = Some(0);
        self
    }

    /// Build a [ChainSpec]
    pub fn build(self) -> ChainSpec {
        ChainSpec {
            chain_id: self.chain_id.expect("The chain id is required"),
            genesis: self.genesis.expect("The genesis is required"),
            genesis_hash: self.genesis_hash.expect("The genesis hash is required"),
            hardforks: self.hardforks,
            paris_block: self.paris_block,
            paris_ttd: self.paris_ttd,
            shanghai_block: self.shanghai_block,
        }
    }
}

/// Merge Status
#[derive(Debug)]
pub enum ParisStatus {
    /// Paris is not supported
    NotSupported,
    /// Paris settings has been set in the chain spec
    Supported {
        /// The merge terminal total difficulty
        terminal_total_difficulty: U256,
        /// The Paris block number
        block: Option<BlockNumber>,
    },
}

impl ParisStatus {
    /// Returns the Paris block number if it is known ahead of time.
    ///
    /// This is only the case for chains that have already activated the merge.
    pub fn block_number(&self) -> Option<BlockNumber> {
        match &self {
            ParisStatus::NotSupported => None,
            ParisStatus::Supported { block, .. } => *block,
        }
    }

    /// Returns the merge terminal total difficulty
    pub fn terminal_total_difficulty(&self) -> Option<U256> {
        match &self {
            ParisStatus::NotSupported => None,
            ParisStatus::Supported { terminal_total_difficulty, .. } => {
                Some(*terminal_total_difficulty)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{Hardfork, MAINNET};

    #[test]
    // this test checks that the forkid computation is accurate
    fn test_forkid_from_hardfork() {
        let frontier_forkid = MAINNET.fork_id(Hardfork::Frontier).unwrap();
        assert_eq!([0xfc, 0x64, 0xec, 0x04], frontier_forkid.hash.0);
        assert_eq!(1150000, frontier_forkid.next);

        let berlin_forkid = MAINNET.fork_id(Hardfork::Berlin).unwrap();
        assert_eq!([0x0e, 0xb4, 0x40, 0xf6], berlin_forkid.hash.0);
        assert_eq!(12965000, berlin_forkid.next);

        let latest_forkid = MAINNET.fork_id(Hardfork::Latest).unwrap();
        assert_eq!(0, latest_forkid.next);
    }
}
