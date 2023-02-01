use crate::{
    constants::EIP1559_INITIAL_BASE_FEE, proofs::genesis_state_root, BlockNumber, Chain,
    ForkFilter, ForkHash, ForkId, Genesis, GenesisAccount, Hardfork, Header, H160, H256, U256,
};
use ethers_core::utils::Genesis as EthersGenesis;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// The Ethereum mainnet spec
pub static MAINNET: Lazy<ChainSpec> = Lazy::new(|| ChainSpec {
    chain: Chain::mainnet(),
    genesis: serde_json::from_str(include_str!("../../res/genesis/mainnet.json"))
        .expect("Can't deserialize Mainnet genesis json"),
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
    dao_fork_support: true,
    paris_block: Some(15537394),
    paris_ttd: Some(U256::from(58750000000000000000000_u128)),
});

/// The Goerli spec
pub static GOERLI: Lazy<ChainSpec> = Lazy::new(|| ChainSpec {
    chain: Chain::goerli(),
    genesis: serde_json::from_str(include_str!("../../res/genesis/goerli.json"))
        .expect("Can't deserialize Goerli genesis json"),
    hardforks: BTreeMap::from([
        (Hardfork::Frontier, 0),
        (Hardfork::Istanbul, 1561651),
        (Hardfork::Berlin, 4460644),
        (Hardfork::London, 5062605),
    ]),
    dao_fork_support: true,
    paris_block: Some(7382818),
    paris_ttd: Some(U256::from(10790000)),
});

/// The Sepolia spec
pub static SEPOLIA: Lazy<ChainSpec> = Lazy::new(|| ChainSpec {
    chain: Chain::sepolia(),
    genesis: serde_json::from_str(include_str!("../../res/genesis/sepolia.json"))
        .expect("Can't deserialize Sepolia genesis json"),
    hardforks: BTreeMap::from([
        (Hardfork::Frontier, 0),
        (Hardfork::Homestead, 0),
        (Hardfork::Dao, 0),
        (Hardfork::Tangerine, 0),
        (Hardfork::SpuriousDragon, 0),
        (Hardfork::Byzantium, 0),
        (Hardfork::Constantinople, 0),
        (Hardfork::Petersburg, 0),
        (Hardfork::Istanbul, 0),
        (Hardfork::Muirglacier, 0),
        (Hardfork::Berlin, 0),
        (Hardfork::London, 0),
        (Hardfork::MergeNetsplit, 1735371),
    ]),
    dao_fork_support: true,
    paris_block: Some(1450408),
    paris_ttd: Some(U256::from(17000000000000000_u64)),
});

/// The Ethereum chain spec
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ChainSpec {
    /// The chain id
    pub chain: Chain,

    /// The genesis block information
    pub genesis: Genesis,

    /// The active hard forks and their block numbers
    pub hardforks: BTreeMap<Hardfork, BlockNumber>,

    /// Whether or not the DAO fork is supported
    pub dao_fork_support: bool,

    /// The block number of the merge
    pub paris_block: Option<u64>,

    /// The merge terminal total difficulty
    pub paris_ttd: Option<U256>,
}

impl ChainSpec {
    /// Returns the chain id
    pub fn chain(&self) -> Chain {
        self.chain
    }

    /// Get the genesis block specification.
    ///
    /// To get the header for the genesis block, use [`Self::genesis_header`] instead.
    pub fn genesis(&self) -> &Genesis {
        &self.genesis
    }

    /// Get the header for the genesis block.
    pub fn genesis_header(&self) -> Header {
        // If London is activated at genesis, we set the initial base fee as per EIP-1559.
        let base_fee_per_gas = if self.fork_active(Hardfork::London, 0) {
            Some(EIP1559_INITIAL_BASE_FEE)
        } else {
            None
        };

        Header {
            gas_limit: self.genesis.gas_limit,
            difficulty: self.genesis.difficulty,
            nonce: self.genesis.nonce,
            extra_data: self.genesis.extra_data.clone(),
            state_root: genesis_state_root(&self.genesis.alloc),
            timestamp: self.genesis.timestamp,
            mix_hash: self.genesis.mix_hash,
            beneficiary: self.genesis.coinbase,
            base_fee_per_gas,
            ..Default::default()
        }
    }

    /// Returns the chain genesis hash
    pub fn genesis_hash(&self) -> H256 {
        self.genesis_header().hash_slow()
    }

    /// Returns the supported hardforks and their fork block numbers
    pub fn hardforks(&self) -> &BTreeMap<Hardfork, BlockNumber> {
        &self.hardforks
    }

    /// Get the first block number of the hardfork.
    pub fn fork_block(&self, fork: Hardfork) -> Option<BlockNumber> {
        self.hardforks.get(&fork).copied()
    }

    /// Returns `true` if the given fork is active on the given block
    pub fn fork_active(&self, fork: Hardfork, current_block: BlockNumber) -> bool {
        self.fork_block(fork).map(|target| target <= current_block).unwrap_or_default()
    }

    /// Returns `true` if the DAO fork is supported
    pub fn dao_fork_support(&self) -> bool {
        self.dao_fork_support
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

    /// Creates a [`ForkFilter`](crate::ForkFilter) for the given [BlockNumber].
    pub fn fork_filter(&self, block: BlockNumber) -> ForkFilter {
        let future_forks =
            self.forks_iter().map(|(_, b)| b).filter(|b| *b > block).collect::<Vec<_>>();

        ForkFilter::new(block, self.genesis_hash(), future_forks)
    }

    /// Compute the forkid for the given [BlockNumber]
    pub fn fork_id(&self, block: BlockNumber) -> ForkId {
        let mut curr_forkhash = ForkHash::from(self.genesis_hash());
        let mut curr_block_number = 0;

        for (_, b) in self.forks_iter() {
            if block >= b {
                if b != curr_block_number {
                    curr_forkhash += b;
                    curr_block_number = b;
                }
            } else {
                return ForkId { hash: curr_forkhash, next: b }
            }
        }
        ForkId { hash: curr_forkhash, next: 0 }
    }

    /// Returns a [ChainSpecBuilder] to help build custom specs
    pub fn builder() -> ChainSpecBuilder {
        ChainSpecBuilder::default()
    }
}

impl From<EthersGenesis> for ChainSpec {
    fn from(genesis: EthersGenesis) -> Self {
        let alloc = genesis
            .alloc
            .iter()
            .map(|(addr, account)| (addr.0.into(), account.clone().into()))
            .collect::<HashMap<H160, GenesisAccount>>();

        let genesis_block = Genesis {
            nonce: genesis.nonce.as_u64(),
            timestamp: genesis.timestamp.as_u64(),
            gas_limit: genesis.gas_limit.as_u64(),
            difficulty: genesis.difficulty.into(),
            mix_hash: genesis.mix_hash.0.into(),
            coinbase: genesis.coinbase.0.into(),
            extra_data: genesis.extra_data.0.into(),
            alloc,
        };

        let paris_ttd = genesis.config.terminal_total_difficulty.map(|ttd| ttd.into());
        let hardfork_opts = vec![
            (Hardfork::Homestead, genesis.config.homestead_block),
            (Hardfork::Dao, genesis.config.dao_fork_block),
            (Hardfork::Tangerine, genesis.config.eip150_block),
            (Hardfork::SpuriousDragon, genesis.config.eip155_block),
            (Hardfork::Byzantium, genesis.config.byzantium_block),
            (Hardfork::Constantinople, genesis.config.constantinople_block),
            (Hardfork::Petersburg, genesis.config.petersburg_block),
            (Hardfork::Istanbul, genesis.config.istanbul_block),
            (Hardfork::Muirglacier, genesis.config.muir_glacier_block),
            (Hardfork::Berlin, genesis.config.berlin_block),
            (Hardfork::London, genesis.config.london_block),
            (Hardfork::ArrowGlacier, genesis.config.arrow_glacier_block),
            (Hardfork::GrayGlacier, genesis.config.gray_glacier_block),
            (Hardfork::MergeNetsplit, genesis.config.merge_netsplit_block),
        ];

        let configured_hardforks = hardfork_opts
            .iter()
            .filter_map(|(hardfork, opt)| opt.map(|block| (*hardfork, block)))
            .collect::<BTreeMap<_, _>>();

        Self {
            chain: genesis.config.chain_id.into(),
            dao_fork_support: genesis.config.dao_fork_support,
            genesis: genesis_block,
            hardforks: configured_hardforks,
            paris_ttd,
            // paris block is not used to fork, and is not used in genesis.json
            paris_block: None,
        }
    }
}

/// A helper to build custom chain specs
#[derive(Debug, Default)]
pub struct ChainSpecBuilder {
    chain: Option<Chain>,
    genesis: Option<Genesis>,
    hardforks: BTreeMap<Hardfork, BlockNumber>,
    dao_fork_support: bool,
    paris_block: Option<u64>,
    paris_ttd: Option<U256>,
}

impl ChainSpecBuilder {
    /// Returns a [ChainSpec] builder initialized with Ethereum mainnet config
    pub fn mainnet() -> Self {
        Self {
            chain: Some(MAINNET.chain),
            genesis: Some(MAINNET.genesis.clone()),
            hardforks: MAINNET.hardforks.clone(),
            dao_fork_support: MAINNET.dao_fork_support,
            paris_block: MAINNET.paris_block,
            paris_ttd: MAINNET.paris_ttd,
        }
    }

    /// Sets the chain id
    pub fn chain(mut self, chain: Chain) -> Self {
        self.chain = Some(chain);
        self
    }

    /// Sets the genesis
    pub fn genesis(mut self, genesis: Genesis) -> Self {
        self.genesis = Some(genesis);
        self
    }

    /// Insert the given fork at the given block number
    pub fn with_fork(mut self, fork: Hardfork, block: BlockNumber) -> Self {
        self.hardforks.insert(fork, block);
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

    /// Sets the DAO fork as supported
    pub fn dao_fork_supported(mut self) -> Self {
        self.dao_fork_support = true;
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
            chain: self.chain.expect("The chain is required"),
            genesis: self.genesis.expect("The genesis is required"),
            hardforks: self.hardforks,
            dao_fork_support: self.dao_fork_support,
            paris_block: self.paris_block,
            paris_ttd: self.paris_ttd,
        }
    }
}

impl From<&ChainSpec> for ChainSpecBuilder {
    fn from(value: &ChainSpec) -> Self {
        Self {
            chain: Some(value.chain),
            genesis: Some(value.genesis.clone()),
            hardforks: value.hardforks.clone(),
            dao_fork_support: value.dao_fork_support,
            paris_block: value.paris_block,
            paris_ttd: value.paris_ttd,
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
    use crate::{Chain, ChainSpec, ForkHash, Genesis, Hardfork, GOERLI, MAINNET, SEPOLIA};

    #[test]
    fn test_empty_forkid() {
        // tests that we skip any forks in block 0, that's the genesis ruleset
        let empty_genesis = Genesis::default();
        let spec = ChainSpec::builder()
            .chain(Chain::mainnet())
            .genesis(empty_genesis)
            .with_fork(Hardfork::Frontier, 0)
            .with_fork(Hardfork::Homestead, 0)
            .with_fork(Hardfork::Tangerine, 0)
            .with_fork(Hardfork::SpuriousDragon, 0)
            .with_fork(Hardfork::Byzantium, 0)
            .with_fork(Hardfork::Constantinople, 0)
            .with_fork(Hardfork::Istanbul, 0)
            .with_fork(Hardfork::Muirglacier, 0)
            .with_fork(Hardfork::Berlin, 0)
            .with_fork(Hardfork::London, 0)
            .with_fork(Hardfork::ArrowGlacier, 0)
            .with_fork(Hardfork::GrayGlacier, 0)
            .build();

        // test at block one - all forks should be active
        let res_forkid = spec.fork_id(1);
        let expected_forkhash = ForkHash::from(spec.genesis_hash());

        // if blocks get activated at genesis then they should not be accumulated into the forkhash
        assert_eq!(res_forkid.hash, expected_forkhash);
    }

    #[test]
    fn test_duplicate_fork_blocks() {
        // forks activated at the same block should be deduplicated
        let empty_genesis = Genesis::default();
        let unique_spec = ChainSpec::builder()
            .chain(Chain::mainnet())
            .genesis(empty_genesis.clone())
            .with_fork(Hardfork::Frontier, 0)
            .with_fork(Hardfork::Homestead, 1)
            .build();

        let duplicate_spec = ChainSpec::builder()
            .chain(Chain::mainnet())
            .genesis(empty_genesis)
            .with_fork(Hardfork::Frontier, 0)
            .with_fork(Hardfork::Homestead, 1)
            .with_fork(Hardfork::Tangerine, 1)
            .build();

        assert_eq!(unique_spec.fork_id(2), duplicate_spec.fork_id(2));
    }

    // these tests check that the forkid computation is accurate
    #[test]
    fn test_mainnet_forkids() {
        let frontier_forkid = MAINNET.fork_id(0);
        assert_eq!([0xfc, 0x64, 0xec, 0x04], frontier_forkid.hash.0);
        assert_eq!(1150000, frontier_forkid.next);

        let homestead_forkid = MAINNET.fork_id(1150000);
        assert_eq!([0x97, 0xc2, 0xc3, 0x4c], homestead_forkid.hash.0);
        assert_eq!(1920000, homestead_forkid.next);

        let dao_forkid = MAINNET.fork_id(1920000);
        assert_eq!([0x91, 0xd1, 0xf9, 0x48], dao_forkid.hash.0);
        assert_eq!(2463000, dao_forkid.next);

        let tangerine_forkid = MAINNET.fork_id(2463000);
        assert_eq!([0x7a, 0x64, 0xda, 0x13], tangerine_forkid.hash.0);
        assert_eq!(2675000, tangerine_forkid.next);

        let spurious_forkid = MAINNET.fork_id(2675000);
        assert_eq!([0x3e, 0xdd, 0x5b, 0x10], spurious_forkid.hash.0);
        assert_eq!(4370000, spurious_forkid.next);

        let byzantium_forkid = MAINNET.fork_id(4370000);
        assert_eq!([0xa0, 0x0b, 0xc3, 0x24], byzantium_forkid.hash.0);
        assert_eq!(7280000, byzantium_forkid.next);

        let constantinople_forkid = MAINNET.fork_id(7280000);
        assert_eq!([0x66, 0x8d, 0xb0, 0xaf], constantinople_forkid.hash.0);
        assert_eq!(9069000, constantinople_forkid.next);

        let istanbul_forkid = MAINNET.fork_id(9069000);
        assert_eq!([0x87, 0x9d, 0x6e, 0x30], istanbul_forkid.hash.0);
        assert_eq!(9200000, istanbul_forkid.next);

        let muir_glacier_forkid = MAINNET.fork_id(9200000);
        assert_eq!([0xe0, 0x29, 0xe9, 0x91], muir_glacier_forkid.hash.0);
        assert_eq!(12244000, muir_glacier_forkid.next);

        let berlin_forkid = MAINNET.fork_id(12244000);
        assert_eq!([0x0e, 0xb4, 0x40, 0xf6], berlin_forkid.hash.0);
        assert_eq!(12965000, berlin_forkid.next);

        let london_forkid = MAINNET.fork_id(12965000);
        assert_eq!([0xb7, 0x15, 0x07, 0x7d], london_forkid.hash.0);
        assert_eq!(13773000, london_forkid.next);

        let arrow_glacier_forkid = MAINNET.fork_id(13773000);
        assert_eq!([0x20, 0xc3, 0x27, 0xfc], arrow_glacier_forkid.hash.0);
        assert_eq!(15050000, arrow_glacier_forkid.next);

        let gray_glacier_forkid = MAINNET.fork_id(15050000);
        assert_eq!([0xf0, 0xaf, 0xd0, 0xe3], gray_glacier_forkid.hash.0);
        assert_eq!(0, gray_glacier_forkid.next); // TODO: update post-gray glacier

        let latest_forkid = MAINNET.fork_id(15050000);
        assert_eq!(0, latest_forkid.next);
    }

    #[test]
    fn test_goerli_forkids() {
        let frontier_forkid = GOERLI.fork_id(0);
        assert_eq!([0xa3, 0xf5, 0xab, 0x08], frontier_forkid.hash.0);
        assert_eq!(1561651, frontier_forkid.next);

        let istanbul_forkid = GOERLI.fork_id(1561651);
        assert_eq!([0xc2, 0x5e, 0xfa, 0x5c], istanbul_forkid.hash.0);
        assert_eq!(4460644, istanbul_forkid.next);

        let berlin_forkid = GOERLI.fork_id(4460644);
        assert_eq!([0x75, 0x7a, 0x1c, 0x47], berlin_forkid.hash.0);
        assert_eq!(5062605, berlin_forkid.next);

        let london_forkid = GOERLI.fork_id(12965000);
        assert_eq!([0xb8, 0xc6, 0x29, 0x9d], london_forkid.hash.0);
        assert_eq!(0, london_forkid.next);
    }

    #[test]
    fn test_sepolia_forkids() {
        // Test vector is from <https://github.com/ethereum/go-ethereum/blob/59a48e0289b1a7470a8285e665cab12b29117a70/core/forkid/forkid_test.go#L146-L151>
        let mergenetsplit_forkid = SEPOLIA.fork_id(1735371);
        assert_eq!([0xb9, 0x6c, 0xbd, 0x13], mergenetsplit_forkid.hash.0);
        assert_eq!(0, mergenetsplit_forkid.next);
    }
}
