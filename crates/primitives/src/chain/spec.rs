use crate::{
    constants::EIP1559_INITIAL_BASE_FEE, forkid::ForkFilterKey, header::Head,
    proofs::genesis_state_root, BlockNumber, Chain, ForkFilter, ForkHash, ForkId, Genesis,
    GenesisAccount, Hardfork, Header, H160, H256, U256,
};
use ethers_core::utils::Genesis as EthersGenesis;
use hex_literal::hex;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// The Ethereum mainnet spec
pub static MAINNET: Lazy<ChainSpec> = Lazy::new(|| ChainSpec {
    chain: Chain::mainnet(),
    genesis: serde_json::from_str(include_str!("../../res/genesis/mainnet.json"))
        .expect("Can't deserialize Mainnet genesis json"),
    genesis_hash: Some(H256(hex!(
        "d4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3"
    ))),
    hardforks: BTreeMap::from([
        (Hardfork::Frontier, ForkCondition::Block(0)),
        (Hardfork::Homestead, ForkCondition::Block(1150000)),
        (Hardfork::Dao, ForkCondition::Block(1920000)),
        (Hardfork::Tangerine, ForkCondition::Block(2463000)),
        (Hardfork::SpuriousDragon, ForkCondition::Block(2675000)),
        (Hardfork::Byzantium, ForkCondition::Block(4370000)),
        (Hardfork::Constantinople, ForkCondition::Block(7280000)),
        (Hardfork::Petersburg, ForkCondition::Block(7280000)),
        (Hardfork::Istanbul, ForkCondition::Block(9069000)),
        (Hardfork::MuirGlacier, ForkCondition::Block(9200000)),
        (Hardfork::Berlin, ForkCondition::Block(12244000)),
        (Hardfork::London, ForkCondition::Block(12965000)),
        (Hardfork::ArrowGlacier, ForkCondition::Block(13773000)),
        (Hardfork::GrayGlacier, ForkCondition::Block(15050000)),
        (
            Hardfork::Paris,
            ForkCondition::TTD {
                fork_block: None,
                total_difficulty: U256::from(58_750_000_000_000_000_000_000_u128),
            },
        ),
    ]),
});

/// The Goerli spec
pub static GOERLI: Lazy<ChainSpec> = Lazy::new(|| ChainSpec {
    chain: Chain::goerli(),
    genesis: serde_json::from_str(include_str!("../../res/genesis/goerli.json"))
        .expect("Can't deserialize Goerli genesis json"),
    genesis_hash: Some(H256(hex!(
        "bf7e331f7f7c1dd2e05159666b3bf8bc7a8a3a9eb1d518969eab529dd9b88c1a"
    ))),
    hardforks: BTreeMap::from([
        (Hardfork::Frontier, ForkCondition::Block(0)),
        (Hardfork::Istanbul, ForkCondition::Block(1561651)),
        (Hardfork::Berlin, ForkCondition::Block(4460644)),
        (Hardfork::London, ForkCondition::Block(5062605)),
        (
            Hardfork::Paris,
            ForkCondition::TTD { fork_block: None, total_difficulty: U256::from(10_790_000) },
        ),
    ]),
});

/// The Sepolia spec
pub static SEPOLIA: Lazy<ChainSpec> = Lazy::new(|| ChainSpec {
    chain: Chain::sepolia(),
    genesis: serde_json::from_str(include_str!("../../res/genesis/sepolia.json"))
        .expect("Can't deserialize Sepolia genesis json"),
    genesis_hash: Some(H256(hex!(
        "25a5cc106eea7138acab33231d7160d69cb777ee0c2c553fcddf5138993e6dd9"
    ))),
    hardforks: BTreeMap::from([
        (Hardfork::Frontier, ForkCondition::Block(0)),
        (Hardfork::Homestead, ForkCondition::Block(0)),
        (Hardfork::Dao, ForkCondition::Block(0)),
        (Hardfork::Tangerine, ForkCondition::Block(0)),
        (Hardfork::SpuriousDragon, ForkCondition::Block(0)),
        (Hardfork::Byzantium, ForkCondition::Block(0)),
        (Hardfork::Constantinople, ForkCondition::Block(0)),
        (Hardfork::Petersburg, ForkCondition::Block(0)),
        (Hardfork::Istanbul, ForkCondition::Block(0)),
        (Hardfork::MuirGlacier, ForkCondition::Block(0)),
        (Hardfork::Berlin, ForkCondition::Block(0)),
        (Hardfork::London, ForkCondition::Block(0)),
        (
            Hardfork::Paris,
            ForkCondition::TTD {
                fork_block: Some(1735371),
                total_difficulty: U256::from(17_000_000_000_000_000u64),
            },
        ),
        (Hardfork::Shanghai, ForkCondition::Timestamp(1677557088)),
    ]),
});

/// An Ethereum chain specification.
///
/// A chain specification describes:
///
/// - Meta-information about the chain (the chain ID)
/// - The genesis block of the chain ([`Genesis`])
/// - What hardforks are activated, and under which conditions
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ChainSpec {
    /// The chain ID
    pub chain: Chain,

    /// The hash of the genesis block.
    ///
    /// This acts as a small cache for known chains. If the chain is known, then the genesis hash
    /// is also known ahead of time, and this will be `Some`.
    #[serde(skip, default)]
    pub genesis_hash: Option<H256>,

    /// The genesis block
    pub genesis: Genesis,

    /// The active hard forks and their activation conditions
    pub hardforks: BTreeMap<Hardfork, ForkCondition>,
}

impl ChainSpec {
    /// Get information about the chain itself
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
        let base_fee_per_gas = if self.fork(Hardfork::London).active_at_block(0) {
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

    /// Get the hash of the genesis block.
    pub fn genesis_hash(&self) -> H256 {
        if let Some(hash) = self.genesis_hash {
            hash
        } else {
            self.genesis_header().hash_slow()
        }
    }

    /// Returns the forks in this specification and their activation conditions.
    pub fn hardforks(&self) -> &BTreeMap<Hardfork, ForkCondition> {
        &self.hardforks
    }

    /// Get the fork condition for the given fork.
    pub fn fork(&self, fork: Hardfork) -> ForkCondition {
        self.hardforks.get(&fork).copied().unwrap_or(ForkCondition::Never)
    }

    /// Get an iterator of all hardforks with their respective activation conditions.
    pub fn forks_iter(&self) -> impl Iterator<Item = (Hardfork, ForkCondition)> + '_ {
        self.hardforks.iter().map(|(f, b)| (*f, *b))
    }

    /// Creates a [`ForkFilter`](crate::ForkFilter) for the block described by [Head].
    pub fn fork_filter(&self, head: Head) -> ForkFilter {
        let forks = self.forks_iter().filter_map(|(_, condition)| {
            // We filter out TTD-based forks w/o a pre-known block since those do not show up in the
            // fork filter.
            Some(match condition {
                ForkCondition::Block(block) => ForkFilterKey::Block(block),
                ForkCondition::Timestamp(time) => ForkFilterKey::Time(time),
                ForkCondition::TTD { fork_block: Some(block), .. } => ForkFilterKey::Block(block),
                _ => return None,
            })
        });

        ForkFilter::new(head, self.genesis_hash(), forks)
    }

    /// Compute the [`ForkId`] for the given [`Head`]
    pub fn fork_id(&self, head: &Head) -> ForkId {
        let mut curr_forkhash = ForkHash::from(self.genesis_hash());
        let mut current_applied_value = 0;

        for (_, cond) in self.forks_iter() {
            let value = match cond {
                ForkCondition::Block(block) => block,
                ForkCondition::Timestamp(time) => time,
                ForkCondition::TTD { fork_block: Some(block), .. } => block,
                _ => continue,
            };

            if cond.active_at_head(head) {
                if value != current_applied_value {
                    curr_forkhash += value;
                    current_applied_value = value;
                }
            } else {
                return ForkId { hash: curr_forkhash, next: value }
            }
        }
        ForkId { hash: curr_forkhash, next: 0 }
    }

    /// Build a chainspec using [`ChainSpecBuilder`]
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

        // Block-based hardforks
        let hardfork_opts = vec![
            (Hardfork::Homestead, genesis.config.homestead_block),
            (Hardfork::Dao, genesis.config.dao_fork_block),
            (Hardfork::Tangerine, genesis.config.eip150_block),
            (Hardfork::SpuriousDragon, genesis.config.eip155_block),
            (Hardfork::Byzantium, genesis.config.byzantium_block),
            (Hardfork::Constantinople, genesis.config.constantinople_block),
            (Hardfork::Petersburg, genesis.config.petersburg_block),
            (Hardfork::Istanbul, genesis.config.istanbul_block),
            (Hardfork::MuirGlacier, genesis.config.muir_glacier_block),
            (Hardfork::Berlin, genesis.config.berlin_block),
            (Hardfork::London, genesis.config.london_block),
            (Hardfork::ArrowGlacier, genesis.config.arrow_glacier_block),
            (Hardfork::GrayGlacier, genesis.config.gray_glacier_block),
        ];
        let mut hardforks = hardfork_opts
            .iter()
            .filter_map(|(hardfork, opt)| opt.map(|block| (*hardfork, ForkCondition::Block(block))))
            .collect::<BTreeMap<_, _>>();

        // Paris
        if let Some(ttd) = genesis.config.terminal_total_difficulty {
            hardforks.insert(
                Hardfork::Paris,
                ForkCondition::TTD {
                    total_difficulty: ttd.into(),
                    fork_block: genesis.config.merge_netsplit_block,
                },
            );
        }

        Self {
            chain: genesis.config.chain_id.into(),
            genesis: genesis_block,
            genesis_hash: None,
            hardforks,
        }
    }
}

/// A helper type for compatibility with geth's config
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum AllGenesisFormats {
    /// The geth genesis format
    Geth(EthersGenesis),
    /// The reth genesis format
    Reth(ChainSpec),
}

impl From<EthersGenesis> for AllGenesisFormats {
    fn from(genesis: EthersGenesis) -> Self {
        Self::Geth(genesis)
    }
}

impl From<ChainSpec> for AllGenesisFormats {
    fn from(genesis: ChainSpec) -> Self {
        Self::Reth(genesis)
    }
}

impl From<AllGenesisFormats> for ChainSpec {
    fn from(genesis: AllGenesisFormats) -> Self {
        match genesis {
            AllGenesisFormats::Geth(genesis) => genesis.into(),
            AllGenesisFormats::Reth(genesis) => genesis,
        }
    }
}

/// A helper to build custom chain specs
#[derive(Debug, Default)]
pub struct ChainSpecBuilder {
    chain: Option<Chain>,
    genesis: Option<Genesis>,
    hardforks: BTreeMap<Hardfork, ForkCondition>,
}

impl ChainSpecBuilder {
    /// Construct a new builder from the mainnet chain spec.
    pub fn mainnet() -> Self {
        Self {
            chain: Some(MAINNET.chain),
            genesis: Some(MAINNET.genesis.clone()),
            hardforks: MAINNET.hardforks.clone(),
        }
    }

    /// Set the chain ID
    pub fn chain(mut self, chain: Chain) -> Self {
        self.chain = Some(chain);
        self
    }

    /// Set the genesis block.
    pub fn genesis(mut self, genesis: Genesis) -> Self {
        self.genesis = Some(genesis);
        self
    }

    /// Add the given fork with the given activation condition to the spec.
    pub fn with_fork(mut self, fork: Hardfork, condition: ForkCondition) -> Self {
        self.hardforks.insert(fork, condition);
        self
    }

    /// Enable Frontier at genesis.
    pub fn frontier_activated(mut self) -> Self {
        self.hardforks.insert(Hardfork::Frontier, ForkCondition::Block(0));
        self
    }

    /// Enable Homestead at genesis.
    pub fn homestead_activated(mut self) -> Self {
        self = self.frontier_activated();
        self.hardforks.insert(Hardfork::Homestead, ForkCondition::Block(0));
        self
    }

    /// Enable Tangerine at genesis.
    pub fn tangerine_whistle_activated(mut self) -> Self {
        self = self.homestead_activated();
        self.hardforks.insert(Hardfork::Tangerine, ForkCondition::Block(0));
        self
    }

    /// Enable Spurious Dragon at genesis.
    pub fn spurious_dragon_activated(mut self) -> Self {
        self = self.tangerine_whistle_activated();
        self.hardforks.insert(Hardfork::SpuriousDragon, ForkCondition::Block(0));
        self
    }

    /// Enable Byzantium at genesis.
    pub fn byzantium_activated(mut self) -> Self {
        self = self.spurious_dragon_activated();
        self.hardforks.insert(Hardfork::Byzantium, ForkCondition::Block(0));
        self
    }

    /// Enable Petersburg at genesis.
    pub fn petersburg_activated(mut self) -> Self {
        self = self.byzantium_activated();
        self.hardforks.insert(Hardfork::Petersburg, ForkCondition::Block(0));
        self
    }

    /// Enable Istanbul at genesis.
    pub fn istanbul_activated(mut self) -> Self {
        self = self.petersburg_activated();
        self.hardforks.insert(Hardfork::Istanbul, ForkCondition::Block(0));
        self
    }

    /// Enable Berlin at genesis.
    pub fn berlin_activated(mut self) -> Self {
        self = self.istanbul_activated();
        self.hardforks.insert(Hardfork::Berlin, ForkCondition::Block(0));
        self
    }

    /// Enable London at genesis.
    pub fn london_activated(mut self) -> Self {
        self = self.berlin_activated();
        self.hardforks.insert(Hardfork::London, ForkCondition::Block(0));
        self
    }

    /// Enable Paris at genesis.
    pub fn paris_activated(mut self) -> Self {
        self = self.london_activated();
        self.hardforks.insert(
            Hardfork::Paris,
            ForkCondition::TTD { fork_block: Some(0), total_difficulty: U256::ZERO },
        );
        self
    }

    /// Enable Shanghai at genesis.
    pub fn shanghai_activated(mut self) -> Self {
        self = self.paris_activated();
        self.hardforks.insert(Hardfork::Shanghai, ForkCondition::Timestamp(0));
        self
    }

    /// Build the resulting [`ChainSpec`].
    ///
    /// # Panics
    ///
    /// This function panics if the chain ID and genesis is not set ([`Self::chain`] and
    /// [`Self::genesis`])
    pub fn build(self) -> ChainSpec {
        ChainSpec {
            chain: self.chain.expect("The chain is required"),
            genesis: self.genesis.expect("The genesis is required"),
            genesis_hash: None,
            hardforks: self.hardforks,
        }
    }
}

impl From<&ChainSpec> for ChainSpecBuilder {
    fn from(value: &ChainSpec) -> Self {
        Self {
            chain: Some(value.chain),
            genesis: Some(value.genesis.clone()),
            hardforks: value.hardforks.clone(),
        }
    }
}

/// The condition at which a fork is activated.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ForkCondition {
    /// The fork is activated after a certain block.
    Block(BlockNumber),
    /// The fork is activated after a total difficulty has been reached.
    TTD {
        /// The block number at which TTD is reached, if it is known.
        ///
        /// This should **NOT** be set unless you want this block advertised as [EIP-2124][eip2124]
        /// `FORK_NEXT`. This is currently only the case for Sepolia.
        ///
        /// [eip2124]: https://eips.ethereum.org/EIPS/eip-2124
        fork_block: Option<BlockNumber>,
        /// The total difficulty after which the fork is activated.
        total_difficulty: U256,
    },
    /// The fork is activated after a specific timestamp.
    Timestamp(u64),
    /// The fork is never activated
    #[default]
    Never,
}

impl ForkCondition {
    /// Checks whether the fork condition is satisfied at the given block.
    ///
    /// For TTD conditions, this will only return true if the activation block is already known.
    ///
    /// For timestamp conditions, this will always return false.
    pub fn active_at_block(&self, current_block: BlockNumber) -> bool {
        match self {
            ForkCondition::Block(block) => current_block >= *block,
            ForkCondition::TTD { fork_block: Some(block), .. } => current_block >= *block,
            _ => false,
        }
    }

    /// Checks if the given block is the first block that satisfies the fork condition.
    ///
    /// This will return false for any condition that is not block based.
    pub fn transitions_at_block(&self, current_block: BlockNumber) -> bool {
        match self {
            ForkCondition::Block(block) => current_block == *block,
            _ => false,
        }
    }

    /// Checks whether the fork condition is satisfied at the given total difficulty and difficulty
    /// of a current block.
    ///
    /// The fork is considered active if the _previous_ total difficulty is above the threshold.
    /// To achieve that, we subtract the passed `difficulty` from the current block's total
    /// difficulty, and check if it's above the Fork Condition's total difficulty (here:
    /// 58_750_000_000_000_000_000_000)
    ///
    /// This will return false for any condition that is not TTD-based.
    pub fn active_at_ttd(&self, ttd: U256, difficulty: U256) -> bool {
        if let ForkCondition::TTD { total_difficulty, .. } = self {
            ttd.saturating_sub(difficulty) >= *total_difficulty
        } else {
            false
        }
    }

    /// Checks whether the fork condition is satisfied at the given timestamp.
    ///
    /// This will return false for any condition that is not timestamp-based.
    pub fn active_at_timestamp(&self, timestamp: u64) -> bool {
        if let ForkCondition::Timestamp(time) = self {
            timestamp >= *time
        } else {
            false
        }
    }

    /// Checks whether the fork condition is satisfied at the given head block.
    ///
    /// This will return true if:
    ///
    /// - The condition is satisfied by the block number;
    /// - The condition is satisfied by the timestamp;
    /// - or the condition is satisfied by the total difficulty
    pub fn active_at_head(&self, head: &Head) -> bool {
        self.active_at_block(head.number) ||
            self.active_at_timestamp(head.timestamp) ||
            self.active_at_ttd(head.total_difficulty, head.difficulty)
    }

    /// Get the total terminal difficulty for this fork condition.
    ///
    /// Returns `None` for fork conditions that are not TTD based.
    pub fn ttd(&self) -> Option<U256> {
        match self {
            ForkCondition::TTD { total_difficulty, .. } => Some(*total_difficulty),
            _ => None,
        }
    }

    /// An internal helper function that gives a value that satisfies this condition.
    pub(crate) fn satisfy(&self) -> Head {
        match *self {
            ForkCondition::Block(number) => Head { number, ..Default::default() },
            ForkCondition::Timestamp(timestamp) => Head { timestamp, ..Default::default() },
            ForkCondition::TTD { total_difficulty, .. } => {
                Head { total_difficulty, ..Default::default() }
            }
            ForkCondition::Never => unreachable!(),
        }
    }
}

#[cfg(test)]
mod tests {
    use revm_primitives::U256;

    use crate::{
        Chain, ChainSpec, ChainSpecBuilder, ForkCondition, ForkHash, ForkId, Genesis, Hardfork,
        Head, GOERLI, MAINNET, SEPOLIA,
    };

    fn test_fork_ids(spec: &ChainSpec, cases: &[(Head, ForkId)]) {
        for (block, expected_id) in cases {
            let computed_id = spec.fork_id(block);
            assert_eq!(
                expected_id, &computed_id,
                "Expected fork ID {:?}, computed fork ID {:?} at block {}",
                expected_id, computed_id, block.number
            );
        }
    }

    // Tests that we skip any fork blocks in block #0 (the genesis ruleset)
    #[test]
    fn ignores_genesis_fork_blocks() {
        let spec = ChainSpec::builder()
            .chain(Chain::mainnet())
            .genesis(Genesis::default())
            .with_fork(Hardfork::Frontier, ForkCondition::Block(0))
            .with_fork(Hardfork::Homestead, ForkCondition::Block(0))
            .with_fork(Hardfork::Tangerine, ForkCondition::Block(0))
            .with_fork(Hardfork::SpuriousDragon, ForkCondition::Block(0))
            .with_fork(Hardfork::Byzantium, ForkCondition::Block(0))
            .with_fork(Hardfork::Constantinople, ForkCondition::Block(0))
            .with_fork(Hardfork::Istanbul, ForkCondition::Block(0))
            .with_fork(Hardfork::MuirGlacier, ForkCondition::Block(0))
            .with_fork(Hardfork::Berlin, ForkCondition::Block(0))
            .with_fork(Hardfork::London, ForkCondition::Block(0))
            .with_fork(Hardfork::ArrowGlacier, ForkCondition::Block(0))
            .with_fork(Hardfork::GrayGlacier, ForkCondition::Block(0))
            .build();

        assert_eq!(spec.hardforks().len(), 12, "12 forks should be active.");
        assert_eq!(
            spec.fork_id(&Head { number: 1, ..Default::default() }),
            ForkId { hash: ForkHash::from(spec.genesis_hash()), next: 0 },
            "the fork ID should be the genesis hash; forks at genesis are ignored for fork filters"
        );
    }

    #[test]
    fn ignores_duplicate_fork_blocks() {
        let empty_genesis = Genesis::default();
        let unique_spec = ChainSpec::builder()
            .chain(Chain::mainnet())
            .genesis(empty_genesis.clone())
            .with_fork(Hardfork::Frontier, ForkCondition::Block(0))
            .with_fork(Hardfork::Homestead, ForkCondition::Block(1))
            .build();

        let duplicate_spec = ChainSpec::builder()
            .chain(Chain::mainnet())
            .genesis(empty_genesis)
            .with_fork(Hardfork::Frontier, ForkCondition::Block(0))
            .with_fork(Hardfork::Homestead, ForkCondition::Block(1))
            .with_fork(Hardfork::Tangerine, ForkCondition::Block(1))
            .build();

        assert_eq!(
            unique_spec.fork_id(&Head { number: 2, ..Default::default() }),
            duplicate_spec.fork_id(&Head { number: 2, ..Default::default() }),
            "duplicate fork blocks should be deduplicated for fork filters"
        );
    }

    #[test]
    fn mainnet_forkids() {
        test_fork_ids(
            &MAINNET,
            &[
                (
                    Head { number: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0xfc, 0x64, 0xec, 0x04]), next: 1150000 },
                ),
                (
                    Head { number: 1150000, ..Default::default() },
                    ForkId { hash: ForkHash([0x97, 0xc2, 0xc3, 0x4c]), next: 1920000 },
                ),
                (
                    Head { number: 1920000, ..Default::default() },
                    ForkId { hash: ForkHash([0x91, 0xd1, 0xf9, 0x48]), next: 2463000 },
                ),
                (
                    Head { number: 2463000, ..Default::default() },
                    ForkId { hash: ForkHash([0x7a, 0x64, 0xda, 0x13]), next: 2675000 },
                ),
                (
                    Head { number: 2675000, ..Default::default() },
                    ForkId { hash: ForkHash([0x3e, 0xdd, 0x5b, 0x10]), next: 4370000 },
                ),
                (
                    Head { number: 4370000, ..Default::default() },
                    ForkId { hash: ForkHash([0xa0, 0x0b, 0xc3, 0x24]), next: 7280000 },
                ),
                (
                    Head { number: 7280000, ..Default::default() },
                    ForkId { hash: ForkHash([0x66, 0x8d, 0xb0, 0xaf]), next: 9069000 },
                ),
                (
                    Head { number: 9069000, ..Default::default() },
                    ForkId { hash: ForkHash([0x87, 0x9d, 0x6e, 0x30]), next: 9200000 },
                ),
                (
                    Head { number: 9200000, ..Default::default() },
                    ForkId { hash: ForkHash([0xe0, 0x29, 0xe9, 0x91]), next: 12244000 },
                ),
                (
                    Head { number: 12244000, ..Default::default() },
                    ForkId { hash: ForkHash([0x0e, 0xb4, 0x40, 0xf6]), next: 12965000 },
                ),
                (
                    Head { number: 12965000, ..Default::default() },
                    ForkId { hash: ForkHash([0xb7, 0x15, 0x07, 0x7d]), next: 13773000 },
                ),
                (
                    Head { number: 13773000, ..Default::default() },
                    ForkId { hash: ForkHash([0x20, 0xc3, 0x27, 0xfc]), next: 15050000 },
                ),
                (
                    Head { number: 15050000, ..Default::default() },
                    ForkId { hash: ForkHash([0xf0, 0xaf, 0xd0, 0xe3]), next: 0 },
                ),
            ],
        );
    }

    #[test]
    fn goerli_forkids() {
        test_fork_ids(
            &GOERLI,
            &[
                (
                    Head { number: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0xa3, 0xf5, 0xab, 0x08]), next: 1561651 },
                ),
                (
                    Head { number: 1561651, ..Default::default() },
                    ForkId { hash: ForkHash([0xc2, 0x5e, 0xfa, 0x5c]), next: 4460644 },
                ),
                (
                    Head { number: 4460644, ..Default::default() },
                    ForkId { hash: ForkHash([0x75, 0x7a, 0x1c, 0x47]), next: 5062605 },
                ),
                (
                    Head { number: 12965000, ..Default::default() },
                    ForkId { hash: ForkHash([0xb8, 0xc6, 0x29, 0x9d]), next: 0 },
                ),
            ],
        );
    }

    #[test]
    fn sepolia_forkids() {
        test_fork_ids(
            &SEPOLIA,
            &[
                (
                    Head { number: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0xfe, 0x33, 0x66, 0xe7]), next: 1735371 },
                ),
                (
                    Head { number: 1735370, ..Default::default() },
                    ForkId { hash: ForkHash([0xfe, 0x33, 0x66, 0xe7]), next: 1735371 },
                ),
                (
                    Head { number: 1735371, ..Default::default() },
                    ForkId { hash: ForkHash([0xb9, 0x6c, 0xbd, 0x13]), next: 1677557088 },
                ),
                (
                    Head { number: 1735372, timestamp: 1677557090, ..Default::default() },
                    ForkId { hash: ForkHash([0xf7, 0xf9, 0xbc, 0x08]), next: 0 },
                ),
            ],
        );
    }

    /// Checks that time-based forks work
    ///
    /// This is based off of the test vectors here: https://github.com/ethereum/go-ethereum/blob/5c8cc10d1e05c23ff1108022f4150749e73c0ca1/core/forkid/forkid_test.go#L155-L188
    #[test]
    fn timestamped_forks() {
        let mainnet_with_shanghai = ChainSpecBuilder::mainnet()
            .with_fork(Hardfork::Shanghai, ForkCondition::Timestamp(1668000000))
            .build();
        test_fork_ids(
            &mainnet_with_shanghai,
            &[
                (
                    Head { number: 0, timestamp: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0xfc, 0x64, 0xec, 0x04]), next: 1150000 },
                ), // Unsynced
                (
                    Head { number: 1149999, timestamp: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0xfc, 0x64, 0xec, 0x04]), next: 1150000 },
                ), // Last Frontier block
                (
                    Head { number: 1150000, timestamp: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0x97, 0xc2, 0xc3, 0x4c]), next: 1920000 },
                ), // First Homestead block
                (
                    Head { number: 1919999, timestamp: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0x97, 0xc2, 0xc3, 0x4c]), next: 1920000 },
                ), // Last Homestead block
                (
                    Head { number: 1920000, timestamp: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0x91, 0xd1, 0xf9, 0x48]), next: 2463000 },
                ), // First DAO block
                (
                    Head { number: 2462999, timestamp: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0x91, 0xd1, 0xf9, 0x48]), next: 2463000 },
                ), // Last DAO block
                (
                    Head { number: 2463000, timestamp: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0x7a, 0x64, 0xda, 0x13]), next: 2675000 },
                ), // First Tangerine block
                (
                    Head { number: 2674999, timestamp: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0x7a, 0x64, 0xda, 0x13]), next: 2675000 },
                ), // Last Tangerine block
                (
                    Head { number: 2675000, timestamp: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0x3e, 0xdd, 0x5b, 0x10]), next: 4370000 },
                ), // First Spurious block
                (
                    Head { number: 4369999, timestamp: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0x3e, 0xdd, 0x5b, 0x10]), next: 4370000 },
                ), // Last Spurious block
                (
                    Head { number: 4370000, timestamp: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0xa0, 0x0b, 0xc3, 0x24]), next: 7280000 },
                ), // First Byzantium block
                (
                    Head { number: 7279999, timestamp: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0xa0, 0x0b, 0xc3, 0x24]), next: 7280000 },
                ), // Last Byzantium block
                (
                    Head { number: 7280000, timestamp: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0x66, 0x8d, 0xb0, 0xaf]), next: 9069000 },
                ), // First and last Constantinople, first Petersburg block
                (
                    Head { number: 9068999, timestamp: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0x66, 0x8d, 0xb0, 0xaf]), next: 9069000 },
                ), // Last Petersburg block
                (
                    Head { number: 9069000, timestamp: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0x87, 0x9d, 0x6e, 0x30]), next: 9200000 },
                ), // First Istanbul and first Muir Glacier block
                (
                    Head { number: 9199999, timestamp: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0x87, 0x9d, 0x6e, 0x30]), next: 9200000 },
                ), // Last Istanbul and first Muir Glacier block
                (
                    Head { number: 9200000, timestamp: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0xe0, 0x29, 0xe9, 0x91]), next: 12244000 },
                ), // First Muir Glacier block
                (
                    Head { number: 12243999, timestamp: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0xe0, 0x29, 0xe9, 0x91]), next: 12244000 },
                ), // Last Muir Glacier block
                (
                    Head { number: 12244000, timestamp: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0x0e, 0xb4, 0x40, 0xf6]), next: 12965000 },
                ), // First Berlin block
                (
                    Head { number: 12964999, timestamp: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0x0e, 0xb4, 0x40, 0xf6]), next: 12965000 },
                ), // Last Berlin block
                (
                    Head { number: 12965000, timestamp: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0xb7, 0x15, 0x07, 0x7d]), next: 13773000 },
                ), // First London block
                (
                    Head { number: 13772999, timestamp: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0xb7, 0x15, 0x07, 0x7d]), next: 13773000 },
                ), // Last London block
                (
                    Head { number: 13773000, timestamp: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0x20, 0xc3, 0x27, 0xfc]), next: 15050000 },
                ), // First Arrow Glacier block
                (
                    Head { number: 15049999, timestamp: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0x20, 0xc3, 0x27, 0xfc]), next: 15050000 },
                ), // Last Arrow Glacier block
                (
                    Head { number: 15050000, timestamp: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0xf0, 0xaf, 0xd0, 0xe3]), next: 1668000000 },
                ), // First Gray Glacier block
                (
                    Head { number: 19999999, timestamp: 1667999999, ..Default::default() },
                    ForkId { hash: ForkHash([0xf0, 0xaf, 0xd0, 0xe3]), next: 1668000000 },
                ), // Last Gray Glacier block
                (
                    Head { number: 20000000, timestamp: 1668000000, ..Default::default() },
                    ForkId { hash: ForkHash([0x71, 0x14, 0x76, 0x44]), next: 0 },
                ), // First Shanghai block
                (
                    Head { number: 20000000, timestamp: 2668000000, ..Default::default() },
                    ForkId { hash: ForkHash([0x71, 0x14, 0x76, 0x44]), next: 0 },
                ), // Future Shanghai block
            ],
        );
    }

    /// Checks that the fork is not active at a terminal ttd block.
    #[test]
    fn check_terminal_ttd() {
        let chainspec = ChainSpecBuilder::mainnet().build();

        // Check that Paris is not active on terminal PoW block #15537393.
        let terminal_block_ttd = U256::from(58750003716598352816469_u128);
        let terminal_block_difficulty = U256::from(11055787484078698_u128);
        assert!(!chainspec
            .fork(Hardfork::Paris)
            .active_at_ttd(terminal_block_ttd, terminal_block_difficulty));

        // Check that Paris is active on first PoS block #15537394.
        let first_pos_block_ttd = U256::from(58750003716598352816469_u128);
        let first_pos_difficulty = U256::ZERO;
        assert!(chainspec
            .fork(Hardfork::Paris)
            .active_at_ttd(first_pos_block_ttd, first_pos_difficulty));
    }
}
