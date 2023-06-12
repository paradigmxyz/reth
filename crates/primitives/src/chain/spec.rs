use crate::{
    constants::{EIP1559_INITIAL_BASE_FEE, EMPTY_WITHDRAWALS},
    forkid::ForkFilterKey,
    header::Head,
    proofs::genesis_state_root,
    BlockNumber, Chain, ForkFilter, ForkHash, ForkId, Genesis, GenesisAccount, Hardfork, Header,
    SealedHeader, H160, H256, U256,
};
use ethers_core::utils::Genesis as EthersGenesis;
use hex_literal::hex;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};



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

    /// The block at which [Hardfork::Paris] was activated and the final difficulty at this block.
    #[serde(skip, default)]
    pub paris_block_and_final_difficulty: Option<(u64, U256)>,

    /// Timestamps of various hardforks
    ///
    /// This caches entries in `hardforks` map
    #[serde(skip, default)]
    pub fork_timestamps: ForkTimestamps,

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
        let base_fee_per_gas = self.initial_base_fee();

        // If shanghai is activated, initialize the header with an empty withdrawals hash, and
        // empty withdrawals list.
        let withdrawals_root =
            (self.fork(Hardfork::Shanghai).active_at_timestamp(self.genesis.timestamp))
                .then_some(EMPTY_WITHDRAWALS);

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
            withdrawals_root,
            ..Default::default()
        }
    }

    /// Get the sealed header for the genesis block.
    pub fn sealed_genesis_header(&self) -> SealedHeader {
        SealedHeader { header: self.genesis_header(), hash: self.genesis_hash() }
    }

    /// Get the initial base fee of the genesis block.
    pub fn initial_base_fee(&self) -> Option<u64> {
        // If London is activated at genesis, we set the initial base fee as per EIP-1559.
        (self.fork(Hardfork::London).active_at_block(0)).then_some(EIP1559_INITIAL_BASE_FEE)
    }

    /// Get the hash of the genesis block.
    pub fn genesis_hash(&self) -> H256 {
        if let Some(hash) = self.genesis_hash {
            hash
        } else {
            self.genesis_header().hash_slow()
        }
    }

    /// Returns the final difficulty if the given block number is after the Paris hardfork.
    ///
    /// Note: technically this would also be valid for the block before the paris upgrade, but this
    /// edge case is omitted here.
    pub fn final_paris_difficulty(&self, block_number: u64) -> Option<U256> {
        self.paris_block_and_final_difficulty.and_then(|(activated_at, final_difficulty)| {
            if block_number >= activated_at {
                Some(final_difficulty)
            } else {
                None
            }
        })
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

    /// Convenience method to check if a fork is active at a given timestamp.
    #[inline]
    pub fn is_fork_active_at_timestamp(&self, fork: Hardfork, timestamp: u64) -> bool {
        self.fork(fork).active_at_timestamp(timestamp)
    }

    /// Convenience method to check if [Hardfork::Shanghai] is active at a given timestamp.
    #[inline]
    pub fn is_shanghai_activated_at_timestamp(&self, timestamp: u64) -> bool {
        self.fork_timestamps
            .shanghai
            .map(|shanghai| timestamp >= shanghai)
            .unwrap_or_else(|| self.is_fork_active_at_timestamp(Hardfork::Shanghai, timestamp))
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

    // Build a chainspec using [`ChainSpecBuilder`]
    // pub fn builder() -> ChainSpecBuilder {
        // ChainSpecBuilder::default()
    // }
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

        // Time-based hardforks
        let time_hardforks = genesis
            .config
            .shanghai_time
            .map(|time| (Hardfork::Shanghai, ForkCondition::Timestamp(time)))
            .into_iter()
            .collect::<BTreeMap<_, _>>();

        hardforks.extend(time_hardforks);

        Self {
            chain: genesis.config.chain_id.into(),
            genesis: genesis_block,
            genesis_hash: None,
            fork_timestamps: ForkTimestamps::from_hardforks(&hardforks),
            hardforks,
            paris_block_and_final_difficulty: None,
        }
    }
}

/// Various timestamps of forks
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct ForkTimestamps {
    /// The timestamp of the shanghai fork
    pub shanghai: Option<u64>,
}

impl ForkTimestamps {
    /// Creates a new [`ForkTimestamps`] from the given hardforks by extracing the timestamps
    fn from_hardforks(forks: &BTreeMap<Hardfork, ForkCondition>) -> Self {
        let mut timestamps = ForkTimestamps::default();
        if let Some(shanghai) = forks.get(&Hardfork::Shanghai).and_then(|f| f.as_timestamp()) {
            timestamps = timestamps.shanghai(shanghai);
        }
        timestamps
    }

    /// Sets the given shanghai timestamp
    pub fn shanghai(mut self, shanghai: u64) -> Self {
        self.shanghai = Some(shanghai);
        self
    }
}

/// A helper type for compatibility with geth's config
#[derive(Debug, Clone, Deserialize, Serialize)]
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

impl From<Arc<ChainSpec>> for AllGenesisFormats {
    fn from(mut genesis: Arc<ChainSpec>) -> Self {
        let cloned_genesis = Arc::make_mut(&mut genesis).clone();
        Self::Reth(cloned_genesis)
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
// #[derive(Debug, Default)]
// pub struct ChainSpecBuilder {
//     chain: Option<Chain>,
//     genesis: Option<Genesis>,
//     hardforks: BTreeMap<Hardfork, ForkCondition>,
// }

// impl ChainSpecBuilder {
//     /// Construct a new builder from the mainnet chain spec.
//     pub fn mainnet() -> Self {
//         Self {
//             chain: Some(MAINNET.chain),
//             genesis: Some(MAINNET.genesis.clone()),
//             hardforks: MAINNET.hardforks.clone(),
//         }
//     }

//     /// Set the chain ID
//     pub fn chain(mut self, chain: Chain) -> Self {
//         self.chain = Some(chain);
//         self
//     }

//     /// Set the genesis block.
//     pub fn genesis(mut self, genesis: Genesis) -> Self {
//         self.genesis = Some(genesis);
//         self
//     }

//     /// Add the given fork with the given activation condition to the spec.
//     pub fn with_fork(mut self, fork: Hardfork, condition: ForkCondition) -> Self {
//         self.hardforks.insert(fork, condition);
//         self
//     }

//     /// Enable the Paris hardfork at the given TTD.
//     ///
//     /// Does not set the merge netsplit block.
//     pub fn paris_at_ttd(self, ttd: U256) -> Self {
//         self.with_fork(
//             Hardfork::Paris,
//             ForkCondition::TTD { total_difficulty: ttd, fork_block: None },
//         )
//     }

//     /// Enable Frontier at genesis.
//     pub fn frontier_activated(mut self) -> Self {
//         self.hardforks.insert(Hardfork::Frontier, ForkCondition::Block(0));
//         self
//     }

//     /// Enable Homestead at genesis.
//     pub fn homestead_activated(mut self) -> Self {
//         self = self.frontier_activated();
//         self.hardforks.insert(Hardfork::Homestead, ForkCondition::Block(0));
//         self
//     }

//     /// Enable Tangerine at genesis.
//     pub fn tangerine_whistle_activated(mut self) -> Self {
//         self = self.homestead_activated();
//         self.hardforks.insert(Hardfork::Tangerine, ForkCondition::Block(0));
//         self
//     }

//     /// Enable Spurious Dragon at genesis.
//     pub fn spurious_dragon_activated(mut self) -> Self {
//         self = self.tangerine_whistle_activated();
//         self.hardforks.insert(Hardfork::SpuriousDragon, ForkCondition::Block(0));
//         self
//     }

//     /// Enable Byzantium at genesis.
//     pub fn byzantium_activated(mut self) -> Self {
//         self = self.spurious_dragon_activated();
//         self.hardforks.insert(Hardfork::Byzantium, ForkCondition::Block(0));
//         self
//     }

//     /// Enable Petersburg at genesis.
//     pub fn petersburg_activated(mut self) -> Self {
//         self = self.byzantium_activated();
//         self.hardforks.insert(Hardfork::Petersburg, ForkCondition::Block(0));
//         self
//     }

//     /// Enable Istanbul at genesis.
//     pub fn istanbul_activated(mut self) -> Self {
//         self = self.petersburg_activated();
//         self.hardforks.insert(Hardfork::Istanbul, ForkCondition::Block(0));
//         self
//     }

//     /// Enable Berlin at genesis.
//     pub fn berlin_activated(mut self) -> Self {
//         self = self.istanbul_activated();
//         self.hardforks.insert(Hardfork::Berlin, ForkCondition::Block(0));
//         self
//     }

//     /// Enable London at genesis.
//     pub fn london_activated(mut self) -> Self {
//         self = self.berlin_activated();
//         self.hardforks.insert(Hardfork::London, ForkCondition::Block(0));
//         self
//     }

//     /// Enable Paris at genesis.
//     pub fn paris_activated(mut self) -> Self {
//         self = self.london_activated();
//         self.hardforks.insert(
//             Hardfork::Paris,
//             ForkCondition::TTD { fork_block: Some(0), total_difficulty: U256::ZERO },
//         );
//         self
//     }

//     /// Enable Shanghai at genesis.
//     pub fn shanghai_activated(mut self) -> Self {
//         self = self.paris_activated();
//         self.hardforks.insert(Hardfork::Shanghai, ForkCondition::Timestamp(0));
//         self
//     }

//     /// Build the resulting [`ChainSpec`].
//     ///
//     /// # Panics
//     ///
//     /// This function panics if the chain ID and genesis is not set ([`Self::chain`] and
//     /// [`Self::genesis`])
//     pub fn build(self) -> ChainSpec {
//         ChainSpec {
//             chain: self.chain.expect("The chain is required"),
//             genesis: self.genesis.expect("The genesis is required"),
//             genesis_hash: None,
//             fork_timestamps: ForkTimestamps::from_hardforks(&self.hardforks),
//             hardforks: self.hardforks,
//             paris_block_and_final_difficulty: None,
//         }
//     }
// }

// impl From<&Arc<ChainSpec>> for ChainSpecBuilder {
//     fn from(value: &Arc<ChainSpec>) -> Self {
//         Self {
//             chain: Some(value.chain),
//             genesis: Some(value.genesis.clone()),
//             hardforks: value.hardforks.clone(),
//         }
//     }
// }

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
    /// Returns true if the fork condition is timestamp based.
    pub fn is_timestamp(&self) -> bool {
        matches!(self, ForkCondition::Timestamp(_))
    }

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

    /// Returns the timestamp of the fork condition, if it is timestamp based.
    pub fn as_timestamp(&self) -> Option<u64> {
        match self {
            ForkCondition::Timestamp(timestamp) => Some(*timestamp),
            _ => None,
        }
    }
}

