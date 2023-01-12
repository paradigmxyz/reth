use hex_literal::hex;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{BlockNumber, ChainId, ForkFilter, ForkHash, ForkId, Genesis, Hardfork, H256, U256};

/// The Ethereum chain spec
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ChainSpec {
    chain_id: ChainId,
    genesis: Genesis,
    genesis_hash: H256,
    hardforks: BTreeMap<Hardfork, BlockNumber>,
    // This is an edge case, maybe we move it into a `Hardforks` struct
    paris_block: Option<u64>,
    paris_ttd: Option<U256>,
    shanghai_block: Option<u64>,
}

impl ChainSpec {
    /// Returns the Etereum mainnet spec
    pub fn mainnet() -> Self {
        Self {
            chain_id: 1,
            genesis: Genesis::default(),
            genesis_hash: H256(hex!(
                "d4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3"
            )),
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
            shanghai_block: None,
        }
    }

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

    /// Get the Paris block number
    pub fn paris_block(&self) -> Option<BlockNumber> {
        self.paris_block
    }

    /// Get the Shanghai block number
    pub fn shanghai_block(&self) -> Option<BlockNumber> {
        self.shanghai_block
    }

    /// Get an iterator of all harforks with theirs respectives block number
    pub fn all_forks_blocks(&self) -> impl Iterator<Item = (Hardfork, BlockNumber)> + '_ {
        self.hardforks.iter().map(|(f, b)| (*f, *b))
    }

    /// Creates a [`ForkFilter`](crate::ForkFilter) for the given hardfork.
    ///
    /// **CAUTION**: This assumes the current hardfork's block number is the current head and uses
    /// all known future hardforks to initialize the filter.
    pub fn fork_filter(&self, fork: Hardfork) -> Option<ForkFilter> {
        if let Some(fork_block) = self.fork_block(fork) {
            let future_forks = self
                .all_forks_blocks()
                .map(|(_, b)| b)
                .filter(|b| *b > fork_block)
                .collect::<Vec<_>>();

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

            println!("Fork block: {:?}", fork_block);
            for (_, b) in self.all_forks_blocks() {
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

    /// Get an iterator of all harforks with theirs respectives block number
    pub fn merge_terminal_total_difficulty(&self) -> Option<U256> {
        self.paris_ttd
    }
}

/// A helper to build custom chain specs
pub struct ChainSpecBuilder;

impl ChainSpecBuilder {
    pub fn new() -> Self {
        todo!()
    }

    pub fn mainnet() -> Self {
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

    pub fn build(&self) -> ChainSpec {
        todo!()
    }
}

#[test]
// this test checks that the forkid computation is accurate
fn test_forkid_from_hardfork() {
    let mainnet = ChainSpec::mainnet();

    let frontier_forkid = mainnet.fork_id(Hardfork::Frontier).unwrap();
    assert_eq!([0xfc, 0x64, 0xec, 0x04], frontier_forkid.hash.0);
    assert_eq!(1150000, frontier_forkid.next);

    let berlin_forkid = mainnet.fork_id(Hardfork::Berlin).unwrap();
    assert_eq!([0x0e, 0xb4, 0x40, 0xf6], berlin_forkid.hash.0);
    assert_eq!(12965000, berlin_forkid.next);

    let latest_forkid = mainnet.fork_id(Hardfork::Latest).unwrap();
    assert_eq!(0, latest_forkid.next);
}
