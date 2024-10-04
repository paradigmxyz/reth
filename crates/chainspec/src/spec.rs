pub use alloy_eips::eip1559::BaseFeeParams;

use alloc::{boxed::Box, sync::Arc, vec::Vec};
use alloy_chains::{Chain, NamedChain};
use alloy_genesis::Genesis;
use alloy_primitives::{address, b256, Address, BlockNumber, B256, U256};
use alloy_trie::EMPTY_ROOT_HASH;
use derive_more::From;
use once_cell::sync::{Lazy, OnceCell};
use reth_ethereum_forks::{
    ChainHardforks, DisplayHardforks, EthereumHardfork, EthereumHardforks, ForkCondition,
    ForkFilter, ForkFilterKey, ForkHash, ForkId, Hardfork, Hardforks, Head, DEV_HARDFORKS,
};
use reth_network_peers::{
    base_nodes, base_testnet_nodes, holesky_nodes, mainnet_nodes, op_nodes, op_testnet_nodes,
    sepolia_nodes, NodeRecord,
};
use reth_primitives_traits::{
    constants::{
        DEV_GENESIS_HASH, EIP1559_INITIAL_BASE_FEE, EMPTY_WITHDRAWALS, ETHEREUM_BLOCK_GAS_LIMIT,
        HOLESKY_GENESIS_HASH, MAINNET_GENESIS_HASH, SEPOLIA_GENESIS_HASH,
    },
    Header, SealedHeader,
};
use reth_trie_common::root::state_root_ref_unhashed;

use crate::{constants::MAINNET_DEPOSIT_CONTRACT, once_cell_set, EthChainSpec};

/// The Ethereum mainnet spec
pub static MAINNET: Lazy<Arc<ChainSpec>> = Lazy::new(|| {
    let mut spec = ChainSpec {
        chain: Chain::mainnet(),
        genesis: serde_json::from_str(include_str!("../res/genesis/mainnet.json"))
            .expect("Can't deserialize Mainnet genesis json"),
        genesis_hash: once_cell_set(MAINNET_GENESIS_HASH),
        genesis_header: Default::default(),
        // <https://etherscan.io/block/15537394>
        paris_block_and_final_difficulty: Some((
            15537394,
            U256::from(58_750_003_716_598_352_816_469u128),
        )),
        hardforks: EthereumHardfork::mainnet().into(),
        // https://etherscan.io/tx/0xe75fb554e433e03763a1560646ee22dcb74e5274b34c5ad644e7c0f619a7e1d0
        deposit_contract: Some(DepositContract::new(
            address!("00000000219ab540356cbb839cbe05303d7705fa"),
            11052984,
            b256!("649bbc62d0e31342afea4e5cd82d4049e7e1ee912fc0889aa790803be39038c5"),
        )),
        base_fee_params: BaseFeeParamsKind::Constant(BaseFeeParams::ethereum()),
        max_gas_limit: ETHEREUM_BLOCK_GAS_LIMIT,
        prune_delete_limit: 20000,
    };
    spec.genesis.config.dao_fork_support = true;
    spec.into()
});

/// The Sepolia spec
pub static SEPOLIA: Lazy<Arc<ChainSpec>> = Lazy::new(|| {
    let mut spec = ChainSpec {
        chain: Chain::sepolia(),
        genesis: serde_json::from_str(include_str!("../res/genesis/sepolia.json"))
            .expect("Can't deserialize Sepolia genesis json"),
        genesis_hash: once_cell_set(SEPOLIA_GENESIS_HASH),
        genesis_header: Default::default(),
        // <https://sepolia.etherscan.io/block/1450409>
        paris_block_and_final_difficulty: Some((1450409, U256::from(17_000_018_015_853_232u128))),
        hardforks: EthereumHardfork::sepolia().into(),
        // https://sepolia.etherscan.io/tx/0x025ecbf81a2f1220da6285d1701dc89fb5a956b62562ee922e1a9efd73eb4b14
        deposit_contract: Some(DepositContract::new(
            address!("7f02c3e3c98b133055b8b348b2ac625669ed295d"),
            1273020,
            b256!("649bbc62d0e31342afea4e5cd82d4049e7e1ee912fc0889aa790803be39038c5"),
        )),
        base_fee_params: BaseFeeParamsKind::Constant(BaseFeeParams::ethereum()),
        max_gas_limit: ETHEREUM_BLOCK_GAS_LIMIT,
        prune_delete_limit: 10000,
    };
    spec.genesis.config.dao_fork_support = true;
    spec.into()
});

/// The Holesky spec
pub static HOLESKY: Lazy<Arc<ChainSpec>> = Lazy::new(|| {
    let mut spec = ChainSpec {
        chain: Chain::holesky(),
        genesis: serde_json::from_str(include_str!("../res/genesis/holesky.json"))
            .expect("Can't deserialize Holesky genesis json"),
        genesis_hash: once_cell_set(HOLESKY_GENESIS_HASH),
        genesis_header: Default::default(),
        paris_block_and_final_difficulty: Some((0, U256::from(1))),
        hardforks: EthereumHardfork::holesky().into(),
        deposit_contract: Some(DepositContract::new(
            address!("4242424242424242424242424242424242424242"),
            0,
            b256!("649bbc62d0e31342afea4e5cd82d4049e7e1ee912fc0889aa790803be39038c5"),
        )),
        base_fee_params: BaseFeeParamsKind::Constant(BaseFeeParams::ethereum()),
        max_gas_limit: ETHEREUM_BLOCK_GAS_LIMIT,
        prune_delete_limit: 10000,
    };
    spec.genesis.config.dao_fork_support = true;
    spec.into()
});

/// Dev testnet specification
///
/// Includes 20 prefunded accounts with `10_000` ETH each derived from mnemonic "test test test test
/// test test test test test test test junk".
pub static DEV: Lazy<Arc<ChainSpec>> = Lazy::new(|| {
    ChainSpec {
        chain: Chain::dev(),
        genesis: serde_json::from_str(include_str!("../res/genesis/dev.json"))
            .expect("Can't deserialize Dev testnet genesis json"),
        genesis_hash: once_cell_set(DEV_GENESIS_HASH),
        paris_block_and_final_difficulty: Some((0, U256::from(0))),
        hardforks: DEV_HARDFORKS.clone(),
        base_fee_params: BaseFeeParamsKind::Constant(BaseFeeParams::ethereum()),
        deposit_contract: None, // TODO: do we even have?
        ..Default::default()
    }
    .into()
});

/// A wrapper around [`BaseFeeParams`] that allows for specifying constant or dynamic EIP-1559
/// parameters based on the active [Hardfork].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BaseFeeParamsKind {
    /// Constant [`BaseFeeParams`]; used for chains that don't have dynamic EIP-1559 parameters
    Constant(BaseFeeParams),
    /// Variable [`BaseFeeParams`]; used for chains that have dynamic EIP-1559 parameters like
    /// Optimism
    Variable(ForkBaseFeeParams),
}

impl Default for BaseFeeParamsKind {
    fn default() -> Self {
        BaseFeeParams::ethereum().into()
    }
}

impl From<BaseFeeParams> for BaseFeeParamsKind {
    fn from(params: BaseFeeParams) -> Self {
        Self::Constant(params)
    }
}

impl From<ForkBaseFeeParams> for BaseFeeParamsKind {
    fn from(params: ForkBaseFeeParams) -> Self {
        Self::Variable(params)
    }
}

/// A type alias to a vector of tuples of [Hardfork] and [`BaseFeeParams`], sorted by [Hardfork]
/// activation order. This is used to specify dynamic EIP-1559 parameters for chains like Optimism.
#[derive(Clone, Debug, PartialEq, Eq, From)]
pub struct ForkBaseFeeParams(Vec<(Box<dyn Hardfork>, BaseFeeParams)>);

impl core::ops::Deref for ChainSpec {
    type Target = ChainHardforks;

    fn deref(&self) -> &Self::Target {
        &self.hardforks
    }
}

/// An Ethereum chain specification.
///
/// A chain specification describes:
///
/// - Meta-information about the chain (the chain ID)
/// - The genesis block of the chain ([`Genesis`])
/// - What hardforks are activated, and under which conditions
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainSpec {
    /// The chain ID
    pub chain: Chain,

    /// The genesis block.
    pub genesis: Genesis,

    /// The hash of the genesis block.
    ///
    /// This is either stored at construction time if it is known using [`once_cell_set`], or
    /// computed once on the first access.
    pub genesis_hash: OnceCell<B256>,

    /// The header corresponding to the genesis block.
    ///
    /// This is either stored at construction time if it is known using [`once_cell_set`], or
    /// computed once on the first access.
    pub genesis_header: OnceCell<Header>,

    /// The block at which [`EthereumHardfork::Paris`] was activated and the final difficulty at
    /// this block.
    pub paris_block_and_final_difficulty: Option<(u64, U256)>,

    /// The active hard forks and their activation conditions
    pub hardforks: ChainHardforks,

    /// The deposit contract deployed for `PoS`
    pub deposit_contract: Option<DepositContract>,

    /// The parameters that configure how a block's base fee is computed
    pub base_fee_params: BaseFeeParamsKind,

    /// The maximum gas limit
    pub max_gas_limit: u64,

    /// The delete limit for pruner, per run.
    pub prune_delete_limit: usize,
}

impl Default for ChainSpec {
    fn default() -> Self {
        Self {
            chain: Default::default(),
            genesis_hash: Default::default(),
            genesis: Default::default(),
            genesis_header: Default::default(),
            paris_block_and_final_difficulty: Default::default(),
            hardforks: Default::default(),
            deposit_contract: Default::default(),
            base_fee_params: BaseFeeParamsKind::Constant(BaseFeeParams::ethereum()),
            max_gas_limit: ETHEREUM_BLOCK_GAS_LIMIT,
            prune_delete_limit: MAINNET.prune_delete_limit,
        }
    }
}

impl ChainSpec {
    /// Get information about the chain itself
    pub const fn chain(&self) -> Chain {
        self.chain
    }

    /// Returns `true` if this chain contains Ethereum configuration.
    #[inline]
    pub const fn is_ethereum(&self) -> bool {
        self.chain.is_ethereum()
    }

    /// Returns `true` if this chain contains Optimism configuration.
    #[inline]
    #[cfg(feature = "optimism")]
    pub fn is_optimism(&self) -> bool {
        self.chain.is_optimism() ||
            self.hardforks.get(reth_optimism_forks::OptimismHardfork::Bedrock).is_some()
    }

    /// Returns `true` if this chain contains Optimism configuration.
    #[inline]
    #[cfg(not(feature = "optimism"))]
    pub const fn is_optimism(&self) -> bool {
        self.chain.is_optimism()
    }

    /// Returns `true` if this chain is Optimism mainnet.
    #[inline]
    pub fn is_optimism_mainnet(&self) -> bool {
        self.chain == Chain::optimism_mainnet()
    }

    /// Get the genesis block specification.
    ///
    /// To get the header for the genesis block, use [`Self::genesis_header`] instead.
    pub const fn genesis(&self) -> &Genesis {
        &self.genesis
    }

    /// Get the header for the genesis block.
    pub fn genesis_header(&self) -> &Header {
        self.genesis_header.get_or_init(|| self.make_genesis_header())
    }

    fn make_genesis_header(&self) -> Header {
        // If London is activated at genesis, we set the initial base fee as per EIP-1559.
        let base_fee_per_gas = self.initial_base_fee();

        // If shanghai is activated, initialize the header with an empty withdrawals hash, and
        // empty withdrawals list.
        let withdrawals_root = self
            .fork(EthereumHardfork::Shanghai)
            .active_at_timestamp(self.genesis.timestamp)
            .then_some(EMPTY_WITHDRAWALS);

        // If Cancun is activated at genesis, we set:
        // * parent beacon block root to 0x0
        // * blob gas used to provided genesis or 0x0
        // * excess blob gas to provided genesis or 0x0
        let (parent_beacon_block_root, blob_gas_used, excess_blob_gas) =
            if self.is_cancun_active_at_timestamp(self.genesis.timestamp) {
                let blob_gas_used = self.genesis.blob_gas_used.unwrap_or(0);
                let excess_blob_gas = self.genesis.excess_blob_gas.unwrap_or(0);
                (Some(B256::ZERO), Some(blob_gas_used as u64), Some(excess_blob_gas as u64))
            } else {
                (None, None, None)
            };

        // If Prague is activated at genesis we set requests root to an empty trie root.
        let requests_root = if self.is_prague_active_at_timestamp(self.genesis.timestamp) {
            Some(EMPTY_ROOT_HASH)
        } else {
            None
        };

        Header {
            gas_limit: self.genesis.gas_limit,
            difficulty: self.genesis.difficulty,
            nonce: self.genesis.nonce.into(),
            extra_data: self.genesis.extra_data.clone(),
            state_root: state_root_ref_unhashed(&self.genesis.alloc),
            timestamp: self.genesis.timestamp,
            mix_hash: self.genesis.mix_hash,
            beneficiary: self.genesis.coinbase,
            base_fee_per_gas: base_fee_per_gas.map(Into::into),
            withdrawals_root,
            parent_beacon_block_root,
            blob_gas_used: blob_gas_used.map(Into::into),
            excess_blob_gas: excess_blob_gas.map(Into::into),
            requests_root,
            ..Default::default()
        }
    }

    /// Get the sealed header for the genesis block.
    pub fn sealed_genesis_header(&self) -> SealedHeader {
        SealedHeader::new(self.genesis_header().clone(), self.genesis_hash())
    }

    /// Get the initial base fee of the genesis block.
    pub fn initial_base_fee(&self) -> Option<u64> {
        // If the base fee is set in the genesis block, we use that instead of the default.
        let genesis_base_fee =
            self.genesis.base_fee_per_gas.map(|fee| fee as u64).unwrap_or(EIP1559_INITIAL_BASE_FEE);

        // If London is activated at genesis, we set the initial base fee as per EIP-1559.
        self.hardforks.fork(EthereumHardfork::London).active_at_block(0).then_some(genesis_base_fee)
    }

    /// Get the [`BaseFeeParams`] for the chain at the given timestamp.
    pub fn base_fee_params_at_timestamp(&self, timestamp: u64) -> BaseFeeParams {
        match self.base_fee_params {
            BaseFeeParamsKind::Constant(bf_params) => bf_params,
            BaseFeeParamsKind::Variable(ForkBaseFeeParams(ref bf_params)) => {
                // Walk through the base fee params configuration in reverse order, and return the
                // first one that corresponds to a hardfork that is active at the
                // given timestamp.
                for (fork, params) in bf_params.iter().rev() {
                    if self.hardforks.is_fork_active_at_timestamp(fork.clone(), timestamp) {
                        return *params
                    }
                }

                bf_params.first().map(|(_, params)| *params).unwrap_or(BaseFeeParams::ethereum())
            }
        }
    }

    /// Get the [`BaseFeeParams`] for the chain at the given block number
    pub fn base_fee_params_at_block(&self, block_number: u64) -> BaseFeeParams {
        match self.base_fee_params {
            BaseFeeParamsKind::Constant(bf_params) => bf_params,
            BaseFeeParamsKind::Variable(ForkBaseFeeParams(ref bf_params)) => {
                // Walk through the base fee params configuration in reverse order, and return the
                // first one that corresponds to a hardfork that is active at the
                // given timestamp.
                for (fork, params) in bf_params.iter().rev() {
                    if self.hardforks.is_fork_active_at_block(fork.clone(), block_number) {
                        return *params
                    }
                }

                bf_params.first().map(|(_, params)| *params).unwrap_or(BaseFeeParams::ethereum())
            }
        }
    }

    /// Get the hash of the genesis block.
    pub fn genesis_hash(&self) -> B256 {
        *self.genesis_hash.get_or_init(|| self.genesis_header().hash_slow())
    }

    /// Get the timestamp of the genesis block.
    pub const fn genesis_timestamp(&self) -> u64 {
        self.genesis.timestamp
    }

    /// Returns the final total difficulty if the Paris hardfork is known.
    pub fn get_final_paris_total_difficulty(&self) -> Option<U256> {
        self.paris_block_and_final_difficulty.map(|(_, final_difficulty)| final_difficulty)
    }

    /// Returns the final total difficulty if the given block number is after the Paris hardfork.
    ///
    /// Note: technically this would also be valid for the block before the paris upgrade, but this
    /// edge case is omitted here.
    #[inline]
    pub fn final_paris_total_difficulty(&self, block_number: u64) -> Option<U256> {
        self.paris_block_and_final_difficulty.and_then(|(activated_at, final_difficulty)| {
            (block_number >= activated_at).then_some(final_difficulty)
        })
    }

    /// Get the fork filter for the given hardfork
    pub fn hardfork_fork_filter<H: Hardfork + Clone>(&self, fork: H) -> Option<ForkFilter> {
        match self.hardforks.fork(fork.clone()) {
            ForkCondition::Never => None,
            _ => Some(self.fork_filter(self.satisfy(self.hardforks.fork(fork)))),
        }
    }

    /// Returns the hardfork display helper.
    pub fn display_hardforks(&self) -> DisplayHardforks {
        DisplayHardforks::new(&self, self.paris_block_and_final_difficulty.map(|(block, _)| block))
    }

    /// Get the fork id for the given hardfork.
    #[inline]
    pub fn hardfork_fork_id<H: Hardfork + Clone>(&self, fork: H) -> Option<ForkId> {
        let condition = self.hardforks.fork(fork);
        match condition {
            ForkCondition::Never => None,
            _ => Some(self.fork_id(&self.satisfy(condition))),
        }
    }

    /// Convenience method to get the fork id for [`EthereumHardfork::Shanghai`] from a given
    /// chainspec.
    #[inline]
    pub fn shanghai_fork_id(&self) -> Option<ForkId> {
        self.hardfork_fork_id(EthereumHardfork::Shanghai)
    }

    /// Convenience method to get the fork id for [`EthereumHardfork::Cancun`] from a given
    /// chainspec.
    #[inline]
    pub fn cancun_fork_id(&self) -> Option<ForkId> {
        self.hardfork_fork_id(EthereumHardfork::Cancun)
    }

    /// Convenience method to get the latest fork id from the chainspec. Panics if chainspec has no
    /// hardforks.
    #[inline]
    pub fn latest_fork_id(&self) -> ForkId {
        self.hardfork_fork_id(self.hardforks.last().unwrap().0).unwrap()
    }

    /// Creates a [`ForkFilter`] for the block described by [Head].
    pub fn fork_filter(&self, head: Head) -> ForkFilter {
        let forks = self.hardforks.forks_iter().filter_map(|(_, condition)| {
            // We filter out TTD-based forks w/o a pre-known block since those do not show up in the
            // fork filter.
            Some(match condition {
                ForkCondition::Block(block) |
                ForkCondition::TTD { fork_block: Some(block), .. } => ForkFilterKey::Block(block),
                ForkCondition::Timestamp(time) => ForkFilterKey::Time(time),
                _ => return None,
            })
        });

        ForkFilter::new(head, self.genesis_hash(), self.genesis_timestamp(), forks)
    }

    /// Compute the [`ForkId`] for the given [`Head`] following eip-6122 spec
    pub fn fork_id(&self, head: &Head) -> ForkId {
        let mut forkhash = ForkHash::from(self.genesis_hash());
        let mut current_applied = 0;

        // handle all block forks before handling timestamp based forks. see: https://eips.ethereum.org/EIPS/eip-6122
        for (_, cond) in self.hardforks.forks_iter() {
            // handle block based forks and the sepolia merge netsplit block edge case (TTD
            // ForkCondition with Some(block))
            if let ForkCondition::Block(block) |
            ForkCondition::TTD { fork_block: Some(block), .. } = cond
            {
                if cond.active_at_head(head) {
                    if block != current_applied {
                        forkhash += block;
                        current_applied = block;
                    }
                } else {
                    // we can return here because this block fork is not active, so we set the
                    // `next` value
                    return ForkId { hash: forkhash, next: block }
                }
            }
        }

        // timestamp are ALWAYS applied after the merge.
        //
        // this filter ensures that no block-based forks are returned
        for timestamp in self.hardforks.forks_iter().filter_map(|(_, cond)| {
            cond.as_timestamp().filter(|time| time > &self.genesis.timestamp)
        }) {
            let cond = ForkCondition::Timestamp(timestamp);
            if cond.active_at_head(head) {
                if timestamp != current_applied {
                    forkhash += timestamp;
                    current_applied = timestamp;
                }
            } else {
                // can safely return here because we have already handled all block forks and
                // have handled all active timestamp forks, and set the next value to the
                // timestamp that is known but not active yet
                return ForkId { hash: forkhash, next: timestamp }
            }
        }

        ForkId { hash: forkhash, next: 0 }
    }

    /// An internal helper function that returns a head block that satisfies a given Fork condition.
    pub(crate) fn satisfy(&self, cond: ForkCondition) -> Head {
        match cond {
            ForkCondition::Block(number) => Head { number, ..Default::default() },
            ForkCondition::Timestamp(timestamp) => {
                // to satisfy every timestamp ForkCondition, we find the last ForkCondition::Block
                // if one exists, and include its block_num in the returned Head
                Head {
                    timestamp,
                    number: self.last_block_fork_before_merge_or_timestamp().unwrap_or_default(),
                    ..Default::default()
                }
            }
            ForkCondition::TTD { total_difficulty, .. } => {
                Head { total_difficulty, ..Default::default() }
            }
            ForkCondition::Never => unreachable!(),
        }
    }

    /// An internal helper function that returns the block number of the last block-based
    /// fork that occurs before any existing TTD (merge)/timestamp based forks.
    ///
    /// Note: this returns None if the `ChainSpec` is not configured with a TTD/Timestamp fork.
    pub(crate) fn last_block_fork_before_merge_or_timestamp(&self) -> Option<u64> {
        let mut hardforks_iter = self.hardforks.forks_iter().peekable();
        while let Some((_, curr_cond)) = hardforks_iter.next() {
            if let Some((_, next_cond)) = hardforks_iter.peek() {
                // peek and find the first occurrence of ForkCondition::TTD (merge) , or in
                // custom ChainSpecs, the first occurrence of
                // ForkCondition::Timestamp. If curr_cond is ForkCondition::Block at
                // this point, which it should be in most "normal" ChainSpecs,
                // return its block_num
                match next_cond {
                    ForkCondition::TTD { fork_block, .. } => {
                        // handle Sepolia merge netsplit case
                        if fork_block.is_some() {
                            return *fork_block
                        }
                        // ensure curr_cond is indeed ForkCondition::Block and return block_num
                        if let ForkCondition::Block(block_num) = curr_cond {
                            return Some(block_num)
                        }
                    }
                    ForkCondition::Timestamp(_) => {
                        // ensure curr_cond is indeed ForkCondition::Block and return block_num
                        if let ForkCondition::Block(block_num) = curr_cond {
                            return Some(block_num)
                        }
                    }
                    ForkCondition::Block(_) | ForkCondition::Never => continue,
                }
            }
        }
        None
    }

    /// Build a chainspec using [`ChainSpecBuilder`]
    pub fn builder() -> ChainSpecBuilder {
        ChainSpecBuilder::default()
    }

    /// Returns the known bootnode records for the given chain.
    pub fn bootnodes(&self) -> Option<Vec<NodeRecord>> {
        use NamedChain as C;
        let chain = self.chain;
        match chain.try_into().ok()? {
            C::Mainnet => Some(mainnet_nodes()),
            C::Sepolia => Some(sepolia_nodes()),
            C::Holesky => Some(holesky_nodes()),
            C::Base => Some(base_nodes()),
            C::Optimism => Some(op_nodes()),
            C::BaseGoerli | C::BaseSepolia => Some(base_testnet_nodes()),
            C::OptimismSepolia | C::OptimismGoerli | C::OptimismKovan => Some(op_testnet_nodes()),
            _ => None,
        }
    }
}

impl From<Genesis> for ChainSpec {
    fn from(genesis: Genesis) -> Self {
        // Block-based hardforks
        let hardfork_opts = [
            (EthereumHardfork::Homestead.boxed(), genesis.config.homestead_block),
            (EthereumHardfork::Dao.boxed(), genesis.config.dao_fork_block),
            (EthereumHardfork::Tangerine.boxed(), genesis.config.eip150_block),
            (EthereumHardfork::SpuriousDragon.boxed(), genesis.config.eip155_block),
            (EthereumHardfork::Byzantium.boxed(), genesis.config.byzantium_block),
            (EthereumHardfork::Constantinople.boxed(), genesis.config.constantinople_block),
            (EthereumHardfork::Petersburg.boxed(), genesis.config.petersburg_block),
            (EthereumHardfork::Istanbul.boxed(), genesis.config.istanbul_block),
            (EthereumHardfork::MuirGlacier.boxed(), genesis.config.muir_glacier_block),
            (EthereumHardfork::Berlin.boxed(), genesis.config.berlin_block),
            (EthereumHardfork::London.boxed(), genesis.config.london_block),
            (EthereumHardfork::ArrowGlacier.boxed(), genesis.config.arrow_glacier_block),
            (EthereumHardfork::GrayGlacier.boxed(), genesis.config.gray_glacier_block),
        ];
        let mut hardforks = hardfork_opts
            .into_iter()
            .filter_map(|(hardfork, opt)| opt.map(|block| (hardfork, ForkCondition::Block(block))))
            .collect::<Vec<_>>();

        // Paris
        let paris_block_and_final_difficulty =
            if let Some(ttd) = genesis.config.terminal_total_difficulty {
                hardforks.push((
                    EthereumHardfork::Paris.boxed(),
                    ForkCondition::TTD {
                        total_difficulty: ttd,
                        fork_block: genesis.config.merge_netsplit_block,
                    },
                ));

                genesis.config.merge_netsplit_block.map(|block| (block, ttd))
            } else {
                None
            };

        // Time-based hardforks
        let time_hardfork_opts = [
            (EthereumHardfork::Shanghai.boxed(), genesis.config.shanghai_time),
            (EthereumHardfork::Cancun.boxed(), genesis.config.cancun_time),
            (EthereumHardfork::Prague.boxed(), genesis.config.prague_time),
        ];

        let mut time_hardforks = time_hardfork_opts
            .into_iter()
            .filter_map(|(hardfork, opt)| {
                opt.map(|time| (hardfork, ForkCondition::Timestamp(time)))
            })
            .collect::<Vec<_>>();

        hardforks.append(&mut time_hardforks);

        // Ordered Hardforks
        let mainnet_hardforks: ChainHardforks = EthereumHardfork::mainnet().into();
        let mainnet_order = mainnet_hardforks.forks_iter();

        let mut ordered_hardforks = Vec::with_capacity(hardforks.len());
        for (hardfork, _) in mainnet_order {
            if let Some(pos) = hardforks.iter().position(|(e, _)| **e == *hardfork) {
                ordered_hardforks.push(hardforks.remove(pos));
            }
        }

        // append the remaining unknown hardforks to ensure we don't filter any out
        ordered_hardforks.append(&mut hardforks);

        // NOTE: in full node, we prune all receipts except the deposit contract's. We do not
        // have the deployment block in the genesis file, so we use block zero. We use the same
        // deposit topic as the mainnet contract if we have the deposit contract address in the
        // genesis json.
        let deposit_contract = genesis.config.deposit_contract_address.map(|address| {
            DepositContract { address, block: 0, topic: MAINNET_DEPOSIT_CONTRACT.topic }
        });

        Self {
            chain: genesis.config.chain_id.into(),
            genesis,
            genesis_hash: OnceCell::new(),
            hardforks: ChainHardforks::new(ordered_hardforks),
            paris_block_and_final_difficulty,
            deposit_contract,
            ..Default::default()
        }
    }
}

impl Hardforks for ChainSpec {
    fn fork<H: Hardfork>(&self, fork: H) -> ForkCondition {
        self.hardforks.fork(fork)
    }

    fn forks_iter(&self) -> impl Iterator<Item = (&dyn Hardfork, ForkCondition)> {
        self.hardforks.forks_iter()
    }

    fn fork_id(&self, head: &Head) -> ForkId {
        self.fork_id(head)
    }

    fn latest_fork_id(&self) -> ForkId {
        self.latest_fork_id()
    }

    fn fork_filter(&self, head: Head) -> ForkFilter {
        self.fork_filter(head)
    }
}

impl EthereumHardforks for ChainSpec {
    fn get_final_paris_total_difficulty(&self) -> Option<U256> {
        self.get_final_paris_total_difficulty()
    }

    fn final_paris_total_difficulty(&self, block_number: u64) -> Option<U256> {
        self.final_paris_total_difficulty(block_number)
    }
}

#[cfg(feature = "optimism")]
impl reth_optimism_forks::OptimismHardforks for ChainSpec {}

/// A trait for reading the current chainspec.
#[auto_impl::auto_impl(&, Arc)]
pub trait ChainSpecProvider: Send + Sync {
    /// The chain spec type.
    type ChainSpec: EthChainSpec + 'static;

    /// Get an [`Arc`] to the chainspec.
    fn chain_spec(&self) -> Arc<Self::ChainSpec>;
}

/// A helper to build custom chain specs
#[derive(Debug, Default, Clone)]
pub struct ChainSpecBuilder {
    chain: Option<Chain>,
    genesis: Option<Genesis>,
    hardforks: ChainHardforks,
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
}

impl ChainSpecBuilder {
    /// Set the chain ID
    pub const fn chain(mut self, chain: Chain) -> Self {
        self.chain = Some(chain);
        self
    }

    /// Set the genesis block.
    pub fn genesis(mut self, genesis: Genesis) -> Self {
        self.genesis = Some(genesis);
        self
    }

    /// Add the given fork with the given activation condition to the spec.
    pub fn with_fork(mut self, fork: EthereumHardfork, condition: ForkCondition) -> Self {
        self.hardforks.insert(fork, condition);
        self
    }

    /// Remove the given fork from the spec.
    pub fn without_fork(mut self, fork: EthereumHardfork) -> Self {
        self.hardforks.remove(fork);
        self
    }

    /// Enable the Paris hardfork at the given TTD.
    ///
    /// Does not set the merge netsplit block.
    pub fn paris_at_ttd(self, ttd: U256) -> Self {
        self.with_fork(
            EthereumHardfork::Paris,
            ForkCondition::TTD { total_difficulty: ttd, fork_block: None },
        )
    }

    /// Enable Frontier at genesis.
    pub fn frontier_activated(mut self) -> Self {
        self.hardforks.insert(EthereumHardfork::Frontier, ForkCondition::Block(0));
        self
    }

    /// Enable Homestead at genesis.
    pub fn homestead_activated(mut self) -> Self {
        self = self.frontier_activated();
        self.hardforks.insert(EthereumHardfork::Homestead, ForkCondition::Block(0));
        self
    }

    /// Enable Tangerine at genesis.
    pub fn tangerine_whistle_activated(mut self) -> Self {
        self = self.homestead_activated();
        self.hardforks.insert(EthereumHardfork::Tangerine, ForkCondition::Block(0));
        self
    }

    /// Enable Spurious Dragon at genesis.
    pub fn spurious_dragon_activated(mut self) -> Self {
        self = self.tangerine_whistle_activated();
        self.hardforks.insert(EthereumHardfork::SpuriousDragon, ForkCondition::Block(0));
        self
    }

    /// Enable Byzantium at genesis.
    pub fn byzantium_activated(mut self) -> Self {
        self = self.spurious_dragon_activated();
        self.hardforks.insert(EthereumHardfork::Byzantium, ForkCondition::Block(0));
        self
    }

    /// Enable Constantinople at genesis.
    pub fn constantinople_activated(mut self) -> Self {
        self = self.byzantium_activated();
        self.hardforks.insert(EthereumHardfork::Constantinople, ForkCondition::Block(0));
        self
    }

    /// Enable Petersburg at genesis.
    pub fn petersburg_activated(mut self) -> Self {
        self = self.constantinople_activated();
        self.hardforks.insert(EthereumHardfork::Petersburg, ForkCondition::Block(0));
        self
    }

    /// Enable Istanbul at genesis.
    pub fn istanbul_activated(mut self) -> Self {
        self = self.petersburg_activated();
        self.hardforks.insert(EthereumHardfork::Istanbul, ForkCondition::Block(0));
        self
    }

    /// Enable Berlin at genesis.
    pub fn berlin_activated(mut self) -> Self {
        self = self.istanbul_activated();
        self.hardforks.insert(EthereumHardfork::Berlin, ForkCondition::Block(0));
        self
    }

    /// Enable London at genesis.
    pub fn london_activated(mut self) -> Self {
        self = self.berlin_activated();
        self.hardforks.insert(EthereumHardfork::London, ForkCondition::Block(0));
        self
    }

    /// Enable Paris at genesis.
    pub fn paris_activated(mut self) -> Self {
        self = self.london_activated();
        self.hardforks.insert(
            EthereumHardfork::Paris,
            ForkCondition::TTD { fork_block: Some(0), total_difficulty: U256::ZERO },
        );
        self
    }

    /// Enable Shanghai at genesis.
    pub fn shanghai_activated(mut self) -> Self {
        self = self.paris_activated();
        self.hardforks.insert(EthereumHardfork::Shanghai, ForkCondition::Timestamp(0));
        self
    }

    /// Enable Cancun at genesis.
    pub fn cancun_activated(mut self) -> Self {
        self = self.shanghai_activated();
        self.hardforks.insert(EthereumHardfork::Cancun, ForkCondition::Timestamp(0));
        self
    }

    /// Enable Prague at genesis.
    pub fn prague_activated(mut self) -> Self {
        self = self.cancun_activated();
        self.hardforks.insert(EthereumHardfork::Prague, ForkCondition::Timestamp(0));
        self
    }

    /// Enable Bedrock at genesis
    #[cfg(feature = "optimism")]
    pub fn bedrock_activated(mut self) -> Self {
        self = self.paris_activated();
        self.hardforks
            .insert(reth_optimism_forks::OptimismHardfork::Bedrock, ForkCondition::Block(0));
        self
    }

    /// Enable Regolith at genesis
    #[cfg(feature = "optimism")]
    pub fn regolith_activated(mut self) -> Self {
        self = self.bedrock_activated();
        self.hardforks
            .insert(reth_optimism_forks::OptimismHardfork::Regolith, ForkCondition::Timestamp(0));
        self
    }

    /// Enable Canyon at genesis
    #[cfg(feature = "optimism")]
    pub fn canyon_activated(mut self) -> Self {
        self = self.regolith_activated();
        // Canyon also activates changes from L1's Shanghai hardfork
        self.hardforks.insert(EthereumHardfork::Shanghai, ForkCondition::Timestamp(0));
        self.hardforks
            .insert(reth_optimism_forks::OptimismHardfork::Canyon, ForkCondition::Timestamp(0));
        self
    }

    /// Enable Ecotone at genesis
    #[cfg(feature = "optimism")]
    pub fn ecotone_activated(mut self) -> Self {
        self = self.canyon_activated();
        self.hardforks.insert(EthereumHardfork::Cancun, ForkCondition::Timestamp(0));
        self.hardforks
            .insert(reth_optimism_forks::OptimismHardfork::Ecotone, ForkCondition::Timestamp(0));
        self
    }

    /// Enable Fjord at genesis
    #[cfg(feature = "optimism")]
    pub fn fjord_activated(mut self) -> Self {
        self = self.ecotone_activated();
        self.hardforks
            .insert(reth_optimism_forks::OptimismHardfork::Fjord, ForkCondition::Timestamp(0));
        self
    }

    /// Enable Granite at genesis
    #[cfg(feature = "optimism")]
    pub fn granite_activated(mut self) -> Self {
        self = self.fjord_activated();
        self.hardforks
            .insert(reth_optimism_forks::OptimismHardfork::Granite, ForkCondition::Timestamp(0));
        self
    }

    /// Build the resulting [`ChainSpec`].
    ///
    /// # Panics
    ///
    /// This function panics if the chain ID and genesis is not set ([`Self::chain`] and
    /// [`Self::genesis`])
    pub fn build(self) -> ChainSpec {
        let paris_block_and_final_difficulty = {
            self.hardforks.get(EthereumHardfork::Paris).and_then(|cond| {
                if let ForkCondition::TTD { fork_block, total_difficulty } = cond {
                    fork_block.map(|fork_block| (fork_block, total_difficulty))
                } else {
                    None
                }
            })
        };
        ChainSpec {
            chain: self.chain.expect("The chain is required"),
            genesis: self.genesis.expect("The genesis is required"),
            genesis_hash: OnceCell::new(),
            hardforks: self.hardforks,
            paris_block_and_final_difficulty,
            deposit_contract: None,
            ..Default::default()
        }
    }
}

impl From<&Arc<ChainSpec>> for ChainSpecBuilder {
    fn from(value: &Arc<ChainSpec>) -> Self {
        Self {
            chain: Some(value.chain),
            genesis: Some(value.genesis.clone()),
            hardforks: value.hardforks.clone(),
        }
    }
}

/// `PoS` deposit contract details.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DepositContract {
    /// Deposit Contract Address
    pub address: Address,
    /// Deployment Block
    pub block: BlockNumber,
    /// `DepositEvent` event signature
    pub topic: B256,
}

impl DepositContract {
    /// Creates a new [`DepositContract`].
    pub const fn new(address: Address, block: BlockNumber, topic: B256) -> Self {
        Self { address, block, topic }
    }
}

/// Verifies [`ChainSpec`] configuration against expected data in given cases.
#[cfg(any(test, feature = "test-utils"))]
pub fn test_fork_ids(spec: &ChainSpec, cases: &[(Head, ForkId)]) {
    for (block, expected_id) in cases {
        let computed_id = spec.fork_id(block);
        assert_eq!(
            expected_id, &computed_id,
            "Expected fork ID {:?}, computed fork ID {:?} at block {}",
            expected_id, computed_id, block.number
        );
    }
}

#[cfg(test)]
mod tests {
    use core::ops::Deref;
    use std::{collections::HashMap, str::FromStr};

    use alloy_chains::Chain;
    use alloy_genesis::{ChainConfig, GenesisAccount};
    use alloy_primitives::{b256, hex};
    use reth_ethereum_forks::{ForkCondition, ForkHash, ForkId, Head};
    use reth_trie_common::TrieAccount;

    use super::*;

    fn test_hardfork_fork_ids(spec: &ChainSpec, cases: &[(EthereumHardfork, ForkId)]) {
        for (hardfork, expected_id) in cases {
            if let Some(computed_id) = spec.hardfork_fork_id(*hardfork) {
                assert_eq!(
                    expected_id, &computed_id,
                    "Expected fork ID {expected_id:?}, computed fork ID {computed_id:?} for hardfork {hardfork}"
                );
                if matches!(hardfork, EthereumHardfork::Shanghai) {
                    if let Some(shangai_id) = spec.shanghai_fork_id() {
                        assert_eq!(
                            expected_id, &shangai_id,
                            "Expected fork ID {expected_id:?}, computed fork ID {computed_id:?} for Shanghai hardfork"
                        );
                    } else {
                        panic!("Expected ForkCondition to return Some for Hardfork::Shanghai");
                    }
                }
            }
        }
    }

    #[test]
    fn test_hardfork_list_display_mainnet() {
        assert_eq!(
            MAINNET.display_hardforks().to_string(),
            "Pre-merge hard forks (block based):
- Frontier                         @0
- Homestead                        @1150000
- Dao                              @1920000
- Tangerine                        @2463000
- SpuriousDragon                   @2675000
- Byzantium                        @4370000
- Constantinople                   @7280000
- Petersburg                       @7280000
- Istanbul                         @9069000
- MuirGlacier                      @9200000
- Berlin                           @12244000
- London                           @12965000
- ArrowGlacier                     @13773000
- GrayGlacier                      @15050000
Merge hard forks:
- Paris                            @58750000000000000000000 (network is known to be merged)
Post-merge hard forks (timestamp based):
- Shanghai                         @1681338455
- Cancun                           @1710338135"
        );
    }

    #[test]
    fn test_hardfork_list_ignores_disabled_forks() {
        let spec = ChainSpec::builder()
            .chain(Chain::mainnet())
            .genesis(Genesis::default())
            .with_fork(EthereumHardfork::Frontier, ForkCondition::Block(0))
            .with_fork(EthereumHardfork::Shanghai, ForkCondition::Never)
            .build();
        assert_eq!(
            spec.display_hardforks().to_string(),
            "Pre-merge hard forks (block based):
- Frontier                         @0"
        );
    }

    // Tests that we skip any fork blocks in block #0 (the genesis ruleset)
    #[test]
    fn ignores_genesis_fork_blocks() {
        let spec = ChainSpec::builder()
            .chain(Chain::mainnet())
            .genesis(Genesis::default())
            .with_fork(EthereumHardfork::Frontier, ForkCondition::Block(0))
            .with_fork(EthereumHardfork::Homestead, ForkCondition::Block(0))
            .with_fork(EthereumHardfork::Tangerine, ForkCondition::Block(0))
            .with_fork(EthereumHardfork::SpuriousDragon, ForkCondition::Block(0))
            .with_fork(EthereumHardfork::Byzantium, ForkCondition::Block(0))
            .with_fork(EthereumHardfork::Constantinople, ForkCondition::Block(0))
            .with_fork(EthereumHardfork::Istanbul, ForkCondition::Block(0))
            .with_fork(EthereumHardfork::MuirGlacier, ForkCondition::Block(0))
            .with_fork(EthereumHardfork::Berlin, ForkCondition::Block(0))
            .with_fork(EthereumHardfork::London, ForkCondition::Block(0))
            .with_fork(EthereumHardfork::ArrowGlacier, ForkCondition::Block(0))
            .with_fork(EthereumHardfork::GrayGlacier, ForkCondition::Block(0))
            .build();

        assert_eq!(spec.deref().len(), 12, "12 forks should be active.");
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
            .with_fork(EthereumHardfork::Frontier, ForkCondition::Block(0))
            .with_fork(EthereumHardfork::Homestead, ForkCondition::Block(1))
            .build();

        let duplicate_spec = ChainSpec::builder()
            .chain(Chain::mainnet())
            .genesis(empty_genesis)
            .with_fork(EthereumHardfork::Frontier, ForkCondition::Block(0))
            .with_fork(EthereumHardfork::Homestead, ForkCondition::Block(1))
            .with_fork(EthereumHardfork::Tangerine, ForkCondition::Block(1))
            .build();

        assert_eq!(
            unique_spec.fork_id(&Head { number: 2, ..Default::default() }),
            duplicate_spec.fork_id(&Head { number: 2, ..Default::default() }),
            "duplicate fork blocks should be deduplicated for fork filters"
        );
    }

    #[test]
    fn test_chainspec_satisfy() {
        let empty_genesis = Genesis::default();
        // happy path test case
        let happy_path_case = ChainSpec::builder()
            .chain(Chain::mainnet())
            .genesis(empty_genesis.clone())
            .with_fork(EthereumHardfork::Frontier, ForkCondition::Block(0))
            .with_fork(EthereumHardfork::Homestead, ForkCondition::Block(73))
            .with_fork(EthereumHardfork::Shanghai, ForkCondition::Timestamp(11313123))
            .build();
        let happy_path_head = happy_path_case.satisfy(ForkCondition::Timestamp(11313123));
        let happy_path_expected = Head { number: 73, timestamp: 11313123, ..Default::default() };
        assert_eq!(
            happy_path_head, happy_path_expected,
            "expected satisfy() to return {happy_path_expected:#?}, but got {happy_path_head:#?} "
        );
        // multiple timestamp test case (i.e Shanghai -> Cancun)
        let multiple_timestamp_fork_case = ChainSpec::builder()
            .chain(Chain::mainnet())
            .genesis(empty_genesis.clone())
            .with_fork(EthereumHardfork::Frontier, ForkCondition::Block(0))
            .with_fork(EthereumHardfork::Homestead, ForkCondition::Block(73))
            .with_fork(EthereumHardfork::Shanghai, ForkCondition::Timestamp(11313123))
            .with_fork(EthereumHardfork::Cancun, ForkCondition::Timestamp(11313398))
            .build();
        let multi_timestamp_head =
            multiple_timestamp_fork_case.satisfy(ForkCondition::Timestamp(11313398));
        let mult_timestamp_expected =
            Head { number: 73, timestamp: 11313398, ..Default::default() };
        assert_eq!(
            multi_timestamp_head, mult_timestamp_expected,
            "expected satisfy() to return {mult_timestamp_expected:#?}, but got {multi_timestamp_head:#?} "
        );
        // no ForkCondition::Block test case
        let no_block_fork_case = ChainSpec::builder()
            .chain(Chain::mainnet())
            .genesis(empty_genesis.clone())
            .with_fork(EthereumHardfork::Shanghai, ForkCondition::Timestamp(11313123))
            .build();
        let no_block_fork_head = no_block_fork_case.satisfy(ForkCondition::Timestamp(11313123));
        let no_block_fork_expected = Head { number: 0, timestamp: 11313123, ..Default::default() };
        assert_eq!(
            no_block_fork_head, no_block_fork_expected,
            "expected satisfy() to return {no_block_fork_expected:#?}, but got {no_block_fork_head:#?} ",
        );
        // spec w/ ForkCondition::TTD with block_num test case (Sepolia merge netsplit edge case)
        let fork_cond_ttd_blocknum_case = ChainSpec::builder()
            .chain(Chain::mainnet())
            .genesis(empty_genesis.clone())
            .with_fork(EthereumHardfork::Frontier, ForkCondition::Block(0))
            .with_fork(EthereumHardfork::Homestead, ForkCondition::Block(73))
            .with_fork(
                EthereumHardfork::Paris,
                ForkCondition::TTD {
                    fork_block: Some(101),
                    total_difficulty: U256::from(10_790_000),
                },
            )
            .with_fork(EthereumHardfork::Shanghai, ForkCondition::Timestamp(11313123))
            .build();
        let fork_cond_ttd_blocknum_head =
            fork_cond_ttd_blocknum_case.satisfy(ForkCondition::Timestamp(11313123));
        let fork_cond_ttd_blocknum_expected =
            Head { number: 101, timestamp: 11313123, ..Default::default() };
        assert_eq!(
            fork_cond_ttd_blocknum_head, fork_cond_ttd_blocknum_expected,
            "expected satisfy() to return {fork_cond_ttd_blocknum_expected:#?}, but got {fork_cond_ttd_blocknum_expected:#?} ",
        );

        // spec w/ only ForkCondition::Block - test the match arm for ForkCondition::Block to ensure
        // no regressions, for these ForkConditions(Block/TTD) - a separate chain spec definition is
        // technically unnecessary - but we include it here for thoroughness
        let fork_cond_block_only_case = ChainSpec::builder()
            .chain(Chain::mainnet())
            .genesis(empty_genesis)
            .with_fork(EthereumHardfork::Frontier, ForkCondition::Block(0))
            .with_fork(EthereumHardfork::Homestead, ForkCondition::Block(73))
            .build();
        let fork_cond_block_only_head = fork_cond_block_only_case.satisfy(ForkCondition::Block(73));
        let fork_cond_block_only_expected = Head { number: 73, ..Default::default() };
        assert_eq!(
            fork_cond_block_only_head, fork_cond_block_only_expected,
            "expected satisfy() to return {fork_cond_block_only_expected:#?}, but got {fork_cond_block_only_head:#?} ",
        );
        // Fork::ConditionTTD test case without a new chain spec to demonstrate ChainSpec::satisfy
        // is independent of ChainSpec for this(these - including ForkCondition::Block) match arm(s)
        let fork_cond_ttd_no_new_spec = fork_cond_block_only_case.satisfy(ForkCondition::TTD {
            fork_block: None,
            total_difficulty: U256::from(10_790_000),
        });
        let fork_cond_ttd_no_new_spec_expected =
            Head { total_difficulty: U256::from(10_790_000), ..Default::default() };
        assert_eq!(
            fork_cond_ttd_no_new_spec, fork_cond_ttd_no_new_spec_expected,
            "expected satisfy() to return {fork_cond_ttd_blocknum_expected:#?}, but got {fork_cond_ttd_blocknum_expected:#?} ",
        );
    }

    #[test]
    fn mainnet_hardfork_fork_ids() {
        test_hardfork_fork_ids(
            &MAINNET,
            &[
                (
                    EthereumHardfork::Frontier,
                    ForkId { hash: ForkHash([0xfc, 0x64, 0xec, 0x04]), next: 1150000 },
                ),
                (
                    EthereumHardfork::Homestead,
                    ForkId { hash: ForkHash([0x97, 0xc2, 0xc3, 0x4c]), next: 1920000 },
                ),
                (
                    EthereumHardfork::Dao,
                    ForkId { hash: ForkHash([0x91, 0xd1, 0xf9, 0x48]), next: 2463000 },
                ),
                (
                    EthereumHardfork::Tangerine,
                    ForkId { hash: ForkHash([0x7a, 0x64, 0xda, 0x13]), next: 2675000 },
                ),
                (
                    EthereumHardfork::SpuriousDragon,
                    ForkId { hash: ForkHash([0x3e, 0xdd, 0x5b, 0x10]), next: 4370000 },
                ),
                (
                    EthereumHardfork::Byzantium,
                    ForkId { hash: ForkHash([0xa0, 0x0b, 0xc3, 0x24]), next: 7280000 },
                ),
                (
                    EthereumHardfork::Constantinople,
                    ForkId { hash: ForkHash([0x66, 0x8d, 0xb0, 0xaf]), next: 9069000 },
                ),
                (
                    EthereumHardfork::Petersburg,
                    ForkId { hash: ForkHash([0x66, 0x8d, 0xb0, 0xaf]), next: 9069000 },
                ),
                (
                    EthereumHardfork::Istanbul,
                    ForkId { hash: ForkHash([0x87, 0x9d, 0x6e, 0x30]), next: 9200000 },
                ),
                (
                    EthereumHardfork::MuirGlacier,
                    ForkId { hash: ForkHash([0xe0, 0x29, 0xe9, 0x91]), next: 12244000 },
                ),
                (
                    EthereumHardfork::Berlin,
                    ForkId { hash: ForkHash([0x0e, 0xb4, 0x40, 0xf6]), next: 12965000 },
                ),
                (
                    EthereumHardfork::London,
                    ForkId { hash: ForkHash([0xb7, 0x15, 0x07, 0x7d]), next: 13773000 },
                ),
                (
                    EthereumHardfork::ArrowGlacier,
                    ForkId { hash: ForkHash([0x20, 0xc3, 0x27, 0xfc]), next: 15050000 },
                ),
                (
                    EthereumHardfork::GrayGlacier,
                    ForkId { hash: ForkHash([0xf0, 0xaf, 0xd0, 0xe3]), next: 1681338455 },
                ),
                (
                    EthereumHardfork::Shanghai,
                    ForkId { hash: ForkHash([0xdc, 0xe9, 0x6c, 0x2d]), next: 1710338135 },
                ),
                (
                    EthereumHardfork::Cancun,
                    ForkId { hash: ForkHash([0x9f, 0x3d, 0x22, 0x54]), next: 0 },
                ),
            ],
        );
    }

    #[test]
    fn sepolia_hardfork_fork_ids() {
        test_hardfork_fork_ids(
            &SEPOLIA,
            &[
                (
                    EthereumHardfork::Frontier,
                    ForkId { hash: ForkHash([0xfe, 0x33, 0x66, 0xe7]), next: 1735371 },
                ),
                (
                    EthereumHardfork::Homestead,
                    ForkId { hash: ForkHash([0xfe, 0x33, 0x66, 0xe7]), next: 1735371 },
                ),
                (
                    EthereumHardfork::Tangerine,
                    ForkId { hash: ForkHash([0xfe, 0x33, 0x66, 0xe7]), next: 1735371 },
                ),
                (
                    EthereumHardfork::SpuriousDragon,
                    ForkId { hash: ForkHash([0xfe, 0x33, 0x66, 0xe7]), next: 1735371 },
                ),
                (
                    EthereumHardfork::Byzantium,
                    ForkId { hash: ForkHash([0xfe, 0x33, 0x66, 0xe7]), next: 1735371 },
                ),
                (
                    EthereumHardfork::Constantinople,
                    ForkId { hash: ForkHash([0xfe, 0x33, 0x66, 0xe7]), next: 1735371 },
                ),
                (
                    EthereumHardfork::Petersburg,
                    ForkId { hash: ForkHash([0xfe, 0x33, 0x66, 0xe7]), next: 1735371 },
                ),
                (
                    EthereumHardfork::Istanbul,
                    ForkId { hash: ForkHash([0xfe, 0x33, 0x66, 0xe7]), next: 1735371 },
                ),
                (
                    EthereumHardfork::Berlin,
                    ForkId { hash: ForkHash([0xfe, 0x33, 0x66, 0xe7]), next: 1735371 },
                ),
                (
                    EthereumHardfork::London,
                    ForkId { hash: ForkHash([0xfe, 0x33, 0x66, 0xe7]), next: 1735371 },
                ),
                (
                    EthereumHardfork::Paris,
                    ForkId { hash: ForkHash([0xb9, 0x6c, 0xbd, 0x13]), next: 1677557088 },
                ),
                (
                    EthereumHardfork::Shanghai,
                    ForkId { hash: ForkHash([0xf7, 0xf9, 0xbc, 0x08]), next: 1706655072 },
                ),
                (
                    EthereumHardfork::Cancun,
                    ForkId { hash: ForkHash([0x88, 0xcf, 0x81, 0xd9]), next: 0 },
                ),
            ],
        );
    }

    #[test]
    fn mainnet_fork_ids() {
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
                    ForkId { hash: ForkHash([0xf0, 0xaf, 0xd0, 0xe3]), next: 1681338455 },
                ),
                // First Shanghai block
                (
                    Head { number: 20000000, timestamp: 1681338455, ..Default::default() },
                    ForkId { hash: ForkHash([0xdc, 0xe9, 0x6c, 0x2d]), next: 1710338135 },
                ),
                // First Cancun block
                (
                    Head { number: 20000001, timestamp: 1710338135, ..Default::default() },
                    ForkId { hash: ForkHash([0x9f, 0x3d, 0x22, 0x54]), next: 0 },
                ),
                // Future Cancun block
                (
                    Head { number: 20000002, timestamp: 2000000000, ..Default::default() },
                    ForkId { hash: ForkHash([0x9f, 0x3d, 0x22, 0x54]), next: 0 },
                ),
            ],
        );
    }

    #[test]
    fn holesky_fork_ids() {
        test_fork_ids(
            &HOLESKY,
            &[
                (
                    Head { number: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0xc6, 0x1a, 0x60, 0x98]), next: 1696000704 },
                ),
                // First MergeNetsplit block
                (
                    Head { number: 123, ..Default::default() },
                    ForkId { hash: ForkHash([0xc6, 0x1a, 0x60, 0x98]), next: 1696000704 },
                ),
                // Last MergeNetsplit block
                (
                    Head { number: 123, timestamp: 1696000703, ..Default::default() },
                    ForkId { hash: ForkHash([0xc6, 0x1a, 0x60, 0x98]), next: 1696000704 },
                ),
                // First Shanghai block
                (
                    Head { number: 123, timestamp: 1696000704, ..Default::default() },
                    ForkId { hash: ForkHash([0xfd, 0x4f, 0x01, 0x6b]), next: 1707305664 },
                ),
                // Last Shanghai block
                (
                    Head { number: 123, timestamp: 1707305663, ..Default::default() },
                    ForkId { hash: ForkHash([0xfd, 0x4f, 0x01, 0x6b]), next: 1707305664 },
                ),
                // First Cancun block
                (
                    Head { number: 123, timestamp: 1707305664, ..Default::default() },
                    ForkId { hash: ForkHash([0x9b, 0x19, 0x2a, 0xd0]), next: 0 },
                ),
            ],
        )
    }

    #[test]
    fn sepolia_fork_ids() {
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
                    Head { number: 1735372, timestamp: 1677557087, ..Default::default() },
                    ForkId { hash: ForkHash([0xb9, 0x6c, 0xbd, 0x13]), next: 1677557088 },
                ),
                // First Shanghai block
                (
                    Head { number: 1735372, timestamp: 1677557088, ..Default::default() },
                    ForkId { hash: ForkHash([0xf7, 0xf9, 0xbc, 0x08]), next: 1706655072 },
                ),
                // Last Shanghai block
                (
                    Head { number: 1735372, timestamp: 1706655071, ..Default::default() },
                    ForkId { hash: ForkHash([0xf7, 0xf9, 0xbc, 0x08]), next: 1706655072 },
                ),
                // First Cancun block
                (
                    Head { number: 1735372, timestamp: 1706655072, ..Default::default() },
                    ForkId { hash: ForkHash([0x88, 0xcf, 0x81, 0xd9]), next: 0 },
                ),
            ],
        );
    }

    #[test]
    fn dev_fork_ids() {
        test_fork_ids(
            &DEV,
            &[(
                Head { number: 0, ..Default::default() },
                ForkId { hash: ForkHash([0x45, 0xb8, 0x36, 0x12]), next: 0 },
            )],
        )
    }

    /// Checks that time-based forks work
    ///
    /// This is based off of the test vectors here: <https://github.com/ethereum/go-ethereum/blob/5c8cc10d1e05c23ff1108022f4150749e73c0ca1/core/forkid/forkid_test.go#L155-L188>
    #[test]
    fn timestamped_forks() {
        let mainnet_with_timestamps = ChainSpecBuilder::mainnet().build();
        test_fork_ids(
            &mainnet_with_timestamps,
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
                    ForkId { hash: ForkHash([0xf0, 0xaf, 0xd0, 0xe3]), next: 1681338455 },
                ), // First Gray Glacier block
                (
                    Head { number: 19999999, timestamp: 1667999999, ..Default::default() },
                    ForkId { hash: ForkHash([0xf0, 0xaf, 0xd0, 0xe3]), next: 1681338455 },
                ), // Last Gray Glacier block
                (
                    Head { number: 20000000, timestamp: 1681338455, ..Default::default() },
                    ForkId { hash: ForkHash([0xdc, 0xe9, 0x6c, 0x2d]), next: 1710338135 },
                ), // Last Shanghai block
                (
                    Head { number: 20000001, timestamp: 1710338134, ..Default::default() },
                    ForkId { hash: ForkHash([0xdc, 0xe9, 0x6c, 0x2d]), next: 1710338135 },
                ), // First Cancun block
                (
                    Head { number: 20000002, timestamp: 1710338135, ..Default::default() },
                    ForkId { hash: ForkHash([0x9f, 0x3d, 0x22, 0x54]), next: 0 },
                ), // Future Cancun block
                (
                    Head { number: 20000003, timestamp: 2000000000, ..Default::default() },
                    ForkId { hash: ForkHash([0x9f, 0x3d, 0x22, 0x54]), next: 0 },
                ),
            ],
        );
    }

    /// Constructs a [`ChainSpec`] with the given [`ChainSpecBuilder`], shanghai, and cancun fork
    /// timestamps.
    fn construct_chainspec(
        builder: ChainSpecBuilder,
        shanghai_time: u64,
        cancun_time: u64,
    ) -> ChainSpec {
        builder
            .with_fork(EthereumHardfork::Shanghai, ForkCondition::Timestamp(shanghai_time))
            .with_fork(EthereumHardfork::Cancun, ForkCondition::Timestamp(cancun_time))
            .build()
    }

    /// Tests that time-based forks which are active at genesis are not included in forkid hash.
    ///
    /// This is based off of the test vectors here:
    /// <https://github.com/ethereum/go-ethereum/blob/2e02c1ffd9dffd1ec9e43c6b66f6c9bd1e556a0b/core/forkid/forkid_test.go#L390-L440>
    #[test]
    fn test_timestamp_fork_in_genesis() {
        let timestamp = 1690475657u64;
        let default_spec_builder = ChainSpecBuilder::default()
            .chain(Chain::from_id(1337))
            .genesis(Genesis::default().with_timestamp(timestamp))
            .paris_activated();

        // test format: (chain spec, expected next value) - the forkhash will be determined by the
        // genesis hash of the constructed chainspec
        let tests = [
            (
                construct_chainspec(default_spec_builder.clone(), timestamp - 1, timestamp + 1),
                timestamp + 1,
            ),
            (
                construct_chainspec(default_spec_builder.clone(), timestamp, timestamp + 1),
                timestamp + 1,
            ),
            (
                construct_chainspec(default_spec_builder, timestamp + 1, timestamp + 2),
                timestamp + 1,
            ),
        ];

        for (spec, expected_timestamp) in tests {
            let got_forkid = spec.fork_id(&Head { number: 0, timestamp: 0, ..Default::default() });
            // This is slightly different from the geth test because we use the shanghai timestamp
            // to determine whether or not to include a withdrawals root in the genesis header.
            // This makes the genesis hash different, and as a result makes the ChainSpec fork hash
            // different.
            let genesis_hash = spec.genesis_hash();
            let expected_forkid =
                ForkId { hash: ForkHash::from(genesis_hash), next: expected_timestamp };
            assert_eq!(got_forkid, expected_forkid);
        }
    }

    /// Checks that the fork is not active at a terminal ttd block.
    #[test]
    fn check_terminal_ttd() {
        let chainspec = ChainSpecBuilder::mainnet().build();

        // Check that Paris is not active on terminal PoW block #15537393.
        let terminal_block_ttd = U256::from(58750003716598352816469_u128);
        let terminal_block_difficulty = U256::from(11055787484078698_u128);
        assert!(!chainspec
            .fork(EthereumHardfork::Paris)
            .active_at_ttd(terminal_block_ttd, terminal_block_difficulty));

        // Check that Paris is active on first PoS block #15537394.
        let first_pos_block_ttd = U256::from(58750003716598352816469_u128);
        let first_pos_difficulty = U256::ZERO;
        assert!(chainspec
            .fork(EthereumHardfork::Paris)
            .active_at_ttd(first_pos_block_ttd, first_pos_difficulty));
    }

    #[test]
    fn geth_genesis_with_shanghai() {
        let geth_genesis = r#"
        {
          "config": {
            "chainId": 1337,
            "homesteadBlock": 0,
            "eip150Block": 0,
            "eip150Hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "eip155Block": 0,
            "eip158Block": 0,
            "byzantiumBlock": 0,
            "constantinopleBlock": 0,
            "petersburgBlock": 0,
            "istanbulBlock": 0,
            "muirGlacierBlock": 0,
            "berlinBlock": 0,
            "londonBlock": 0,
            "arrowGlacierBlock": 0,
            "grayGlacierBlock": 0,
            "shanghaiTime": 0,
            "cancunTime": 1,
            "terminalTotalDifficulty": 0,
            "terminalTotalDifficultyPassed": true,
            "ethash": {}
          },
          "nonce": "0x0",
          "timestamp": "0x0",
          "extraData": "0x",
          "gasLimit": "0x4c4b40",
          "difficulty": "0x1",
          "mixHash": "0x0000000000000000000000000000000000000000000000000000000000000000",
          "coinbase": "0x0000000000000000000000000000000000000000",
          "alloc": {
            "658bdf435d810c91414ec09147daa6db62406379": {
              "balance": "0x487a9a304539440000"
            },
            "aa00000000000000000000000000000000000000": {
              "code": "0x6042",
              "storage": {
                "0x0000000000000000000000000000000000000000000000000000000000000000": "0x0000000000000000000000000000000000000000000000000000000000000000",
                "0x0100000000000000000000000000000000000000000000000000000000000000": "0x0100000000000000000000000000000000000000000000000000000000000000",
                "0x0200000000000000000000000000000000000000000000000000000000000000": "0x0200000000000000000000000000000000000000000000000000000000000000",
                "0x0300000000000000000000000000000000000000000000000000000000000000": "0x0000000000000000000000000000000000000000000000000000000000000303"
              },
              "balance": "0x1",
              "nonce": "0x1"
            },
            "bb00000000000000000000000000000000000000": {
              "code": "0x600154600354",
              "storage": {
                "0x0000000000000000000000000000000000000000000000000000000000000000": "0x0000000000000000000000000000000000000000000000000000000000000000",
                "0x0100000000000000000000000000000000000000000000000000000000000000": "0x0100000000000000000000000000000000000000000000000000000000000000",
                "0x0200000000000000000000000000000000000000000000000000000000000000": "0x0200000000000000000000000000000000000000000000000000000000000000",
                "0x0300000000000000000000000000000000000000000000000000000000000000": "0x0000000000000000000000000000000000000000000000000000000000000303"
              },
              "balance": "0x2",
              "nonce": "0x1"
            }
          },
          "number": "0x0",
          "gasUsed": "0x0",
          "parentHash": "0x0000000000000000000000000000000000000000000000000000000000000000",
          "baseFeePerGas": "0x3b9aca00"
        }
        "#;

        let genesis: Genesis = serde_json::from_str(geth_genesis).unwrap();
        let chainspec = ChainSpec::from(genesis);

        // assert a bunch of hardforks that should be set
        assert_eq!(
            chainspec.hardforks.get(EthereumHardfork::Homestead).unwrap(),
            ForkCondition::Block(0)
        );
        assert_eq!(
            chainspec.hardforks.get(EthereumHardfork::Tangerine).unwrap(),
            ForkCondition::Block(0)
        );
        assert_eq!(
            chainspec.hardforks.get(EthereumHardfork::SpuriousDragon).unwrap(),
            ForkCondition::Block(0)
        );
        assert_eq!(
            chainspec.hardforks.get(EthereumHardfork::Byzantium).unwrap(),
            ForkCondition::Block(0)
        );
        assert_eq!(
            chainspec.hardforks.get(EthereumHardfork::Constantinople).unwrap(),
            ForkCondition::Block(0)
        );
        assert_eq!(
            chainspec.hardforks.get(EthereumHardfork::Petersburg).unwrap(),
            ForkCondition::Block(0)
        );
        assert_eq!(
            chainspec.hardforks.get(EthereumHardfork::Istanbul).unwrap(),
            ForkCondition::Block(0)
        );
        assert_eq!(
            chainspec.hardforks.get(EthereumHardfork::MuirGlacier).unwrap(),
            ForkCondition::Block(0)
        );
        assert_eq!(
            chainspec.hardforks.get(EthereumHardfork::Berlin).unwrap(),
            ForkCondition::Block(0)
        );
        assert_eq!(
            chainspec.hardforks.get(EthereumHardfork::London).unwrap(),
            ForkCondition::Block(0)
        );
        assert_eq!(
            chainspec.hardforks.get(EthereumHardfork::ArrowGlacier).unwrap(),
            ForkCondition::Block(0)
        );
        assert_eq!(
            chainspec.hardforks.get(EthereumHardfork::GrayGlacier).unwrap(),
            ForkCondition::Block(0)
        );

        // including time based hardforks
        assert_eq!(
            chainspec.hardforks.get(EthereumHardfork::Shanghai).unwrap(),
            ForkCondition::Timestamp(0)
        );

        // including time based hardforks
        assert_eq!(
            chainspec.hardforks.get(EthereumHardfork::Cancun).unwrap(),
            ForkCondition::Timestamp(1)
        );

        // alloc key -> expected rlp mapping
        let key_rlp = vec![
            (hex!("658bdf435d810c91414ec09147daa6db62406379"), &hex!("f84d8089487a9a304539440000a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470")[..]),
            (hex!("aa00000000000000000000000000000000000000"), &hex!("f8440101a08afc95b7d18a226944b9c2070b6bda1c3a36afcc3730429d47579c94b9fe5850a0ce92c756baff35fa740c3557c1a971fd24d2d35b7c8e067880d50cd86bb0bc99")[..]),
            (hex!("bb00000000000000000000000000000000000000"), &hex!("f8440102a08afc95b7d18a226944b9c2070b6bda1c3a36afcc3730429d47579c94b9fe5850a0e25a53cbb501cec2976b393719c63d832423dd70a458731a0b64e4847bbca7d2")[..]),
        ];

        for (key, expected_rlp) in key_rlp {
            let account = chainspec.genesis.alloc.get(&key).expect("account should exist");
            assert_eq!(&alloy_rlp::encode(TrieAccount::from(account.clone())), expected_rlp);
        }

        assert_eq!(chainspec.genesis_hash.get(), None);
        let expected_state_root: B256 =
            hex!("078dc6061b1d8eaa8493384b59c9c65ceb917201221d08b80c4de6770b6ec7e7").into();
        assert_eq!(chainspec.genesis_header().state_root, expected_state_root);

        let expected_withdrawals_hash: B256 =
            hex!("56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421").into();
        assert_eq!(chainspec.genesis_header().withdrawals_root, Some(expected_withdrawals_hash));

        let expected_hash: B256 =
            hex!("1fc027d65f820d3eef441ebeec139ebe09e471cf98516dce7b5643ccb27f418c").into();
        let hash = chainspec.genesis_hash();
        assert_eq!(hash, expected_hash);
    }

    #[test]
    fn hive_geth_json() {
        let hive_json = r#"
        {
            "nonce": "0x0000000000000042",
            "difficulty": "0x2123456",
            "mixHash": "0x123456789abcdef123456789abcdef123456789abcdef123456789abcdef1234",
            "coinbase": "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "timestamp": "0x123456",
            "parentHash": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "extraData": "0xfafbfcfd",
            "gasLimit": "0x2fefd8",
            "alloc": {
                "dbdbdb2cbd23b783741e8d7fcf51e459b497e4a6": {
                    "balance": "0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
                },
                "e6716f9544a56c530d868e4bfbacb172315bdead": {
                    "balance": "0x11",
                    "code": "0x12"
                },
                "b9c015918bdaba24b4ff057a92a3873d6eb201be": {
                    "balance": "0x21",
                    "storage": {
                        "0x0000000000000000000000000000000000000000000000000000000000000001": "0x22"
                    }
                },
                "1a26338f0d905e295fccb71fa9ea849ffa12aaf4": {
                    "balance": "0x31",
                    "nonce": "0x32"
                },
                "0000000000000000000000000000000000000001": {
                    "balance": "0x41"
                },
                "0000000000000000000000000000000000000002": {
                    "balance": "0x51"
                },
                "0000000000000000000000000000000000000003": {
                    "balance": "0x61"
                },
                "0000000000000000000000000000000000000004": {
                    "balance": "0x71"
                }
            },
            "config": {
                "ethash": {},
                "chainId": 10,
                "homesteadBlock": 0,
                "eip150Block": 0,
                "eip155Block": 0,
                "eip158Block": 0,
                "byzantiumBlock": 0,
                "constantinopleBlock": 0,
                "petersburgBlock": 0,
                "istanbulBlock": 0
            }
        }
        "#;

        let genesis = serde_json::from_str::<Genesis>(hive_json).unwrap();
        let chainspec: ChainSpec = genesis.into();
        assert_eq!(chainspec.genesis_hash.get(), None);
        assert_eq!(chainspec.chain, Chain::from_named(NamedChain::Optimism));
        let expected_state_root: B256 =
            hex!("9a6049ac535e3dc7436c189eaa81c73f35abd7f282ab67c32944ff0301d63360").into();
        assert_eq!(chainspec.genesis_header().state_root, expected_state_root);
        let hard_forks = vec![
            EthereumHardfork::Byzantium,
            EthereumHardfork::Homestead,
            EthereumHardfork::Istanbul,
            EthereumHardfork::Petersburg,
            EthereumHardfork::Constantinople,
        ];
        for fork in hard_forks {
            assert_eq!(chainspec.hardforks.get(fork).unwrap(), ForkCondition::Block(0));
        }

        let expected_hash: B256 =
            hex!("5ae31c6522bd5856129f66be3d582b842e4e9faaa87f21cce547128339a9db3c").into();
        let hash = chainspec.genesis_header().hash_slow();
        assert_eq!(hash, expected_hash);
    }

    #[test]
    fn test_hive_paris_block_genesis_json() {
        // this tests that we can handle `parisBlock` in the genesis json and can use it to output
        // a correct forkid
        let hive_paris = r#"
        {
          "config": {
            "ethash": {},
            "chainId": 3503995874084926,
            "homesteadBlock": 0,
            "eip150Block": 6,
            "eip155Block": 12,
            "eip158Block": 12,
            "byzantiumBlock": 18,
            "constantinopleBlock": 24,
            "petersburgBlock": 30,
            "istanbulBlock": 36,
            "muirGlacierBlock": 42,
            "berlinBlock": 48,
            "londonBlock": 54,
            "arrowGlacierBlock": 60,
            "grayGlacierBlock": 66,
            "mergeNetsplitBlock": 72,
            "terminalTotalDifficulty": 9454784,
            "shanghaiTime": 780,
            "cancunTime": 840
          },
          "nonce": "0x0",
          "timestamp": "0x0",
          "extraData": "0x68697665636861696e",
          "gasLimit": "0x23f3e20",
          "difficulty": "0x20000",
          "mixHash": "0x0000000000000000000000000000000000000000000000000000000000000000",
          "coinbase": "0x0000000000000000000000000000000000000000",
          "alloc": {
            "000f3df6d732807ef1319fb7b8bb8522d0beac02": {
              "code": "0x3373fffffffffffffffffffffffffffffffffffffffe14604d57602036146024575f5ffd5b5f35801560495762001fff810690815414603c575f5ffd5b62001fff01545f5260205ff35b5f5ffd5b62001fff42064281555f359062001fff015500",
              "balance": "0x2a"
            },
            "0c2c51a0990aee1d73c1228de158688341557508": {
              "balance": "0xc097ce7bc90715b34b9f1000000000"
            },
            "14e46043e63d0e3cdcf2530519f4cfaf35058cb2": {
              "balance": "0xc097ce7bc90715b34b9f1000000000"
            },
            "16c57edf7fa9d9525378b0b81bf8a3ced0620c1c": {
              "balance": "0xc097ce7bc90715b34b9f1000000000"
            },
            "1f4924b14f34e24159387c0a4cdbaa32f3ddb0cf": {
              "balance": "0xc097ce7bc90715b34b9f1000000000"
            },
            "1f5bde34b4afc686f136c7a3cb6ec376f7357759": {
              "balance": "0xc097ce7bc90715b34b9f1000000000"
            },
            "2d389075be5be9f2246ad654ce152cf05990b209": {
              "balance": "0xc097ce7bc90715b34b9f1000000000"
            },
            "3ae75c08b4c907eb63a8960c45b86e1e9ab6123c": {
              "balance": "0xc097ce7bc90715b34b9f1000000000"
            },
            "4340ee1b812acb40a1eb561c019c327b243b92df": {
              "balance": "0xc097ce7bc90715b34b9f1000000000"
            },
            "4a0f1452281bcec5bd90c3dce6162a5995bfe9df": {
              "balance": "0xc097ce7bc90715b34b9f1000000000"
            },
            "4dde844b71bcdf95512fb4dc94e84fb67b512ed8": {
              "balance": "0xc097ce7bc90715b34b9f1000000000"
            },
            "5f552da00dfb4d3749d9e62dcee3c918855a86a0": {
              "balance": "0xc097ce7bc90715b34b9f1000000000"
            },
            "654aa64f5fbefb84c270ec74211b81ca8c44a72e": {
              "balance": "0xc097ce7bc90715b34b9f1000000000"
            },
            "717f8aa2b982bee0e29f573d31df288663e1ce16": {
              "balance": "0xc097ce7bc90715b34b9f1000000000"
            },
            "7435ed30a8b4aeb0877cef0c6e8cffe834eb865f": {
              "balance": "0xc097ce7bc90715b34b9f1000000000"
            },
            "83c7e323d189f18725ac510004fdc2941f8c4a78": {
              "balance": "0xc097ce7bc90715b34b9f1000000000"
            },
            "84e75c28348fb86acea1a93a39426d7d60f4cc46": {
              "balance": "0xc097ce7bc90715b34b9f1000000000"
            },
            "8bebc8ba651aee624937e7d897853ac30c95a067": {
              "storage": {
                "0x0000000000000000000000000000000000000000000000000000000000000001": "0x0000000000000000000000000000000000000000000000000000000000000001",
                "0x0000000000000000000000000000000000000000000000000000000000000002": "0x0000000000000000000000000000000000000000000000000000000000000002",
                "0x0000000000000000000000000000000000000000000000000000000000000003": "0x0000000000000000000000000000000000000000000000000000000000000003"
              },
              "balance": "0x1",
              "nonce": "0x1"
            },
            "c7b99a164efd027a93f147376cc7da7c67c6bbe0": {
              "balance": "0xc097ce7bc90715b34b9f1000000000"
            },
            "d803681e487e6ac18053afc5a6cd813c86ec3e4d": {
              "balance": "0xc097ce7bc90715b34b9f1000000000"
            },
            "e7d13f7aa2a838d24c59b40186a0aca1e21cffcc": {
              "balance": "0xc097ce7bc90715b34b9f1000000000"
            },
            "eda8645ba6948855e3b3cd596bbb07596d59c603": {
              "balance": "0xc097ce7bc90715b34b9f1000000000"
            }
          },
          "number": "0x0",
          "gasUsed": "0x0",
          "parentHash": "0x0000000000000000000000000000000000000000000000000000000000000000",
          "baseFeePerGas": null,
          "excessBlobGas": null,
          "blobGasUsed": null
        }
        "#;

        // check that it deserializes properly
        let genesis: Genesis = serde_json::from_str(hive_paris).unwrap();
        let chainspec = ChainSpec::from(genesis);

        // make sure we are at ForkHash("bc0c2605") with Head post-cancun
        let expected_forkid = ForkId { hash: ForkHash([0xbc, 0x0c, 0x26, 0x05]), next: 0 };
        let got_forkid =
            chainspec.fork_id(&Head { number: 73, timestamp: 840, ..Default::default() });

        // check that they're the same
        assert_eq!(got_forkid, expected_forkid);
        // Check that paris block and final difficulty are set correctly
        assert_eq!(chainspec.paris_block_and_final_difficulty, Some((72, U256::from(9454784))));
    }

    #[test]
    fn test_parse_genesis_json() {
        let s = r#"{"config":{"ethash":{},"chainId":1337,"homesteadBlock":0,"eip150Block":0,"eip155Block":0,"eip158Block":0,"byzantiumBlock":0,"constantinopleBlock":0,"petersburgBlock":0,"istanbulBlock":0,"berlinBlock":0,"londonBlock":0,"terminalTotalDifficulty":0,"terminalTotalDifficultyPassed":true,"shanghaiTime":0},"nonce":"0x0","timestamp":"0x0","extraData":"0x","gasLimit":"0x4c4b40","difficulty":"0x1","mixHash":"0x0000000000000000000000000000000000000000000000000000000000000000","coinbase":"0x0000000000000000000000000000000000000000","alloc":{"658bdf435d810c91414ec09147daa6db62406379":{"balance":"0x487a9a304539440000"},"aa00000000000000000000000000000000000000":{"code":"0x6042","storage":{"0x0000000000000000000000000000000000000000000000000000000000000000":"0x0000000000000000000000000000000000000000000000000000000000000000","0x0100000000000000000000000000000000000000000000000000000000000000":"0x0100000000000000000000000000000000000000000000000000000000000000","0x0200000000000000000000000000000000000000000000000000000000000000":"0x0200000000000000000000000000000000000000000000000000000000000000","0x0300000000000000000000000000000000000000000000000000000000000000":"0x0000000000000000000000000000000000000000000000000000000000000303"},"balance":"0x1","nonce":"0x1"},"bb00000000000000000000000000000000000000":{"code":"0x600154600354","storage":{"0x0000000000000000000000000000000000000000000000000000000000000000":"0x0000000000000000000000000000000000000000000000000000000000000000","0x0100000000000000000000000000000000000000000000000000000000000000":"0x0100000000000000000000000000000000000000000000000000000000000000","0x0200000000000000000000000000000000000000000000000000000000000000":"0x0200000000000000000000000000000000000000000000000000000000000000","0x0300000000000000000000000000000000000000000000000000000000000000":"0x0000000000000000000000000000000000000000000000000000000000000303"},"balance":"0x2","nonce":"0x1"}},"number":"0x0","gasUsed":"0x0","parentHash":"0x0000000000000000000000000000000000000000000000000000000000000000","baseFeePerGas":"0x1337"}"#;
        let genesis: Genesis = serde_json::from_str(s).unwrap();
        let acc = genesis
            .alloc
            .get(&"0xaa00000000000000000000000000000000000000".parse::<Address>().unwrap())
            .unwrap();
        assert_eq!(acc.balance, U256::from(1));
        assert_eq!(genesis.base_fee_per_gas, Some(0x1337));
    }

    #[test]
    fn test_parse_cancun_genesis_json() {
        let s = r#"{"config":{"ethash":{},"chainId":1337,"homesteadBlock":0,"eip150Block":0,"eip155Block":0,"eip158Block":0,"byzantiumBlock":0,"constantinopleBlock":0,"petersburgBlock":0,"istanbulBlock":0,"berlinBlock":0,"londonBlock":0,"terminalTotalDifficulty":0,"terminalTotalDifficultyPassed":true,"shanghaiTime":0,"cancunTime":4661},"nonce":"0x0","timestamp":"0x0","extraData":"0x","gasLimit":"0x4c4b40","difficulty":"0x1","mixHash":"0x0000000000000000000000000000000000000000000000000000000000000000","coinbase":"0x0000000000000000000000000000000000000000","alloc":{"658bdf435d810c91414ec09147daa6db62406379":{"balance":"0x487a9a304539440000"},"aa00000000000000000000000000000000000000":{"code":"0x6042","storage":{"0x0000000000000000000000000000000000000000000000000000000000000000":"0x0000000000000000000000000000000000000000000000000000000000000000","0x0100000000000000000000000000000000000000000000000000000000000000":"0x0100000000000000000000000000000000000000000000000000000000000000","0x0200000000000000000000000000000000000000000000000000000000000000":"0x0200000000000000000000000000000000000000000000000000000000000000","0x0300000000000000000000000000000000000000000000000000000000000000":"0x0000000000000000000000000000000000000000000000000000000000000303"},"balance":"0x1","nonce":"0x1"},"bb00000000000000000000000000000000000000":{"code":"0x600154600354","storage":{"0x0000000000000000000000000000000000000000000000000000000000000000":"0x0000000000000000000000000000000000000000000000000000000000000000","0x0100000000000000000000000000000000000000000000000000000000000000":"0x0100000000000000000000000000000000000000000000000000000000000000","0x0200000000000000000000000000000000000000000000000000000000000000":"0x0200000000000000000000000000000000000000000000000000000000000000","0x0300000000000000000000000000000000000000000000000000000000000000":"0x0000000000000000000000000000000000000000000000000000000000000303"},"balance":"0x2","nonce":"0x1"}},"number":"0x0","gasUsed":"0x0","parentHash":"0x0000000000000000000000000000000000000000000000000000000000000000","baseFeePerGas":"0x3b9aca00"}"#;
        let genesis: Genesis = serde_json::from_str(s).unwrap();
        let acc = genesis
            .alloc
            .get(&"0xaa00000000000000000000000000000000000000".parse::<Address>().unwrap())
            .unwrap();
        assert_eq!(acc.balance, U256::from(1));
        // assert that the cancun time was picked up
        assert_eq!(genesis.config.cancun_time, Some(4661));
    }

    #[test]
    fn test_parse_prague_genesis_all_formats() {
        let s = r#"{"config":{"ethash":{},"chainId":1337,"homesteadBlock":0,"eip150Block":0,"eip155Block":0,"eip158Block":0,"byzantiumBlock":0,"constantinopleBlock":0,"petersburgBlock":0,"istanbulBlock":0,"berlinBlock":0,"londonBlock":0,"terminalTotalDifficulty":0,"terminalTotalDifficultyPassed":true,"shanghaiTime":0,"cancunTime":4661, "pragueTime": 4662},"nonce":"0x0","timestamp":"0x0","extraData":"0x","gasLimit":"0x4c4b40","difficulty":"0x1","mixHash":"0x0000000000000000000000000000000000000000000000000000000000000000","coinbase":"0x0000000000000000000000000000000000000000","alloc":{"658bdf435d810c91414ec09147daa6db62406379":{"balance":"0x487a9a304539440000"},"aa00000000000000000000000000000000000000":{"code":"0x6042","storage":{"0x0000000000000000000000000000000000000000000000000000000000000000":"0x0000000000000000000000000000000000000000000000000000000000000000","0x0100000000000000000000000000000000000000000000000000000000000000":"0x0100000000000000000000000000000000000000000000000000000000000000","0x0200000000000000000000000000000000000000000000000000000000000000":"0x0200000000000000000000000000000000000000000000000000000000000000","0x0300000000000000000000000000000000000000000000000000000000000000":"0x0000000000000000000000000000000000000000000000000000000000000303"},"balance":"0x1","nonce":"0x1"},"bb00000000000000000000000000000000000000":{"code":"0x600154600354","storage":{"0x0000000000000000000000000000000000000000000000000000000000000000":"0x0000000000000000000000000000000000000000000000000000000000000000","0x0100000000000000000000000000000000000000000000000000000000000000":"0x0100000000000000000000000000000000000000000000000000000000000000","0x0200000000000000000000000000000000000000000000000000000000000000":"0x0200000000000000000000000000000000000000000000000000000000000000","0x0300000000000000000000000000000000000000000000000000000000000000":"0x0000000000000000000000000000000000000000000000000000000000000303"},"balance":"0x2","nonce":"0x1"}},"number":"0x0","gasUsed":"0x0","parentHash":"0x0000000000000000000000000000000000000000000000000000000000000000","baseFeePerGas":"0x3b9aca00"}"#;
        let genesis: Genesis = serde_json::from_str(s).unwrap();

        // assert that the alloc was picked up
        let acc = genesis
            .alloc
            .get(&"0xaa00000000000000000000000000000000000000".parse::<Address>().unwrap())
            .unwrap();
        assert_eq!(acc.balance, U256::from(1));
        // assert that the cancun time was picked up
        assert_eq!(genesis.config.cancun_time, Some(4661));
        // assert that the prague time was picked up
        assert_eq!(genesis.config.prague_time, Some(4662));
    }

    #[test]
    fn test_parse_cancun_genesis_all_formats() {
        let s = r#"{"config":{"ethash":{},"chainId":1337,"homesteadBlock":0,"eip150Block":0,"eip155Block":0,"eip158Block":0,"byzantiumBlock":0,"constantinopleBlock":0,"petersburgBlock":0,"istanbulBlock":0,"berlinBlock":0,"londonBlock":0,"terminalTotalDifficulty":0,"terminalTotalDifficultyPassed":true,"shanghaiTime":0,"cancunTime":4661},"nonce":"0x0","timestamp":"0x0","extraData":"0x","gasLimit":"0x4c4b40","difficulty":"0x1","mixHash":"0x0000000000000000000000000000000000000000000000000000000000000000","coinbase":"0x0000000000000000000000000000000000000000","alloc":{"658bdf435d810c91414ec09147daa6db62406379":{"balance":"0x487a9a304539440000"},"aa00000000000000000000000000000000000000":{"code":"0x6042","storage":{"0x0000000000000000000000000000000000000000000000000000000000000000":"0x0000000000000000000000000000000000000000000000000000000000000000","0x0100000000000000000000000000000000000000000000000000000000000000":"0x0100000000000000000000000000000000000000000000000000000000000000","0x0200000000000000000000000000000000000000000000000000000000000000":"0x0200000000000000000000000000000000000000000000000000000000000000","0x0300000000000000000000000000000000000000000000000000000000000000":"0x0000000000000000000000000000000000000000000000000000000000000303"},"balance":"0x1","nonce":"0x1"},"bb00000000000000000000000000000000000000":{"code":"0x600154600354","storage":{"0x0000000000000000000000000000000000000000000000000000000000000000":"0x0000000000000000000000000000000000000000000000000000000000000000","0x0100000000000000000000000000000000000000000000000000000000000000":"0x0100000000000000000000000000000000000000000000000000000000000000","0x0200000000000000000000000000000000000000000000000000000000000000":"0x0200000000000000000000000000000000000000000000000000000000000000","0x0300000000000000000000000000000000000000000000000000000000000000":"0x0000000000000000000000000000000000000000000000000000000000000303"},"balance":"0x2","nonce":"0x1"}},"number":"0x0","gasUsed":"0x0","parentHash":"0x0000000000000000000000000000000000000000000000000000000000000000","baseFeePerGas":"0x3b9aca00"}"#;
        let genesis: Genesis = serde_json::from_str(s).unwrap();

        // assert that the alloc was picked up
        let acc = genesis
            .alloc
            .get(&"0xaa00000000000000000000000000000000000000".parse::<Address>().unwrap())
            .unwrap();
        assert_eq!(acc.balance, U256::from(1));
        // assert that the cancun time was picked up
        assert_eq!(genesis.config.cancun_time, Some(4661));
    }

    #[test]
    fn test_paris_block_and_total_difficulty() {
        let genesis = Genesis { gas_limit: 0x2fefd8u64, ..Default::default() };
        let paris_chainspec = ChainSpecBuilder::default()
            .chain(Chain::from_id(1337))
            .genesis(genesis)
            .paris_activated()
            .build();
        assert_eq!(paris_chainspec.paris_block_and_final_difficulty, Some((0, U256::ZERO)));
    }

    #[test]
    fn test_default_cancun_header_forkhash() {
        // set the gas limit from the hive test genesis according to the hash
        let genesis = Genesis { gas_limit: 0x2fefd8u64, ..Default::default() };
        let default_chainspec = ChainSpecBuilder::default()
            .chain(Chain::from_id(1337))
            .genesis(genesis)
            .cancun_activated()
            .build();
        let mut header = default_chainspec.genesis_header().clone();

        // set the state root to the same as in the hive test the hash was pulled from
        header.state_root =
            B256::from_str("0x62e2595e017f0ca23e08d17221010721a71c3ae932f4ea3cb12117786bb392d4")
                .unwrap();

        // shanghai is activated so we should have a withdrawals root
        assert_eq!(header.withdrawals_root, Some(EMPTY_WITHDRAWALS));

        // cancun is activated so we should have a zero parent beacon block root, zero blob gas
        // used, and zero excess blob gas
        assert_eq!(header.parent_beacon_block_root, Some(B256::ZERO));
        assert_eq!(header.blob_gas_used, Some(0));
        assert_eq!(header.excess_blob_gas, Some(0));

        // check the genesis hash
        let genesis_hash = header.hash_slow();
        let expected_hash =
            b256!("16bb7c59613a5bad3f7c04a852fd056545ade2483968d9a25a1abb05af0c4d37");
        assert_eq!(genesis_hash, expected_hash);

        // check that the forkhash is correct
        let expected_forkhash = ForkHash(hex!("8062457a"));
        assert_eq!(ForkHash::from(genesis_hash), expected_forkhash);
    }

    #[test]
    fn holesky_paris_activated_at_genesis() {
        assert!(HOLESKY
            .fork(EthereumHardfork::Paris)
            .active_at_ttd(HOLESKY.genesis.difficulty, HOLESKY.genesis.difficulty));
    }

    #[test]
    fn test_genesis_format_deserialization() {
        // custom genesis with chain config
        let config = ChainConfig {
            chain_id: 2600,
            homestead_block: Some(0),
            eip150_block: Some(0),
            eip155_block: Some(0),
            eip158_block: Some(0),
            byzantium_block: Some(0),
            constantinople_block: Some(0),
            petersburg_block: Some(0),
            istanbul_block: Some(0),
            berlin_block: Some(0),
            london_block: Some(0),
            shanghai_time: Some(0),
            terminal_total_difficulty: Some(U256::ZERO),
            terminal_total_difficulty_passed: true,
            ..Default::default()
        };
        // genesis
        let genesis = Genesis {
            config,
            nonce: 0,
            timestamp: 1698688670,
            gas_limit: 5000,
            difficulty: U256::ZERO,
            mix_hash: B256::ZERO,
            coinbase: Address::ZERO,
            ..Default::default()
        };

        // seed accounts after genesis struct created
        let address = hex!("6Be02d1d3665660d22FF9624b7BE0551ee1Ac91b").into();
        let account = GenesisAccount::default().with_balance(U256::from(33));
        let genesis = genesis.extend_accounts(HashMap::from([(address, account)]));

        // ensure genesis is deserialized correctly
        let serialized_genesis = serde_json::to_string(&genesis).unwrap();
        let deserialized_genesis: Genesis = serde_json::from_str(&serialized_genesis).unwrap();

        assert_eq!(genesis, deserialized_genesis);
    }

    #[test]
    fn check_fork_id_chainspec_with_fork_condition_never() {
        let spec = ChainSpec {
            chain: Chain::mainnet(),
            genesis: Genesis::default(),
            genesis_hash: OnceCell::new(),
            hardforks: ChainHardforks::new(vec![(
                EthereumHardfork::Frontier.boxed(),
                ForkCondition::Never,
            )]),
            paris_block_and_final_difficulty: None,
            deposit_contract: None,
            ..Default::default()
        };

        assert_eq!(spec.hardfork_fork_id(EthereumHardfork::Frontier), None);
    }

    #[test]
    fn check_fork_filter_chainspec_with_fork_condition_never() {
        let spec = ChainSpec {
            chain: Chain::mainnet(),
            genesis: Genesis::default(),
            genesis_hash: OnceCell::new(),
            hardforks: ChainHardforks::new(vec![(
                EthereumHardfork::Shanghai.boxed(),
                ForkCondition::Never,
            )]),
            paris_block_and_final_difficulty: None,
            deposit_contract: None,
            ..Default::default()
        };

        assert_eq!(spec.hardfork_fork_filter(EthereumHardfork::Shanghai), None);
    }

    #[test]
    fn latest_eth_mainnet_fork_id() {
        assert_eq!(
            ForkId { hash: ForkHash([0x9f, 0x3d, 0x22, 0x54]), next: 0 },
            MAINNET.latest_fork_id()
        )
    }

    #[test]
    fn test_fork_order_ethereum_mainnet() {
        let genesis = Genesis {
            config: ChainConfig {
                chain_id: 0,
                homestead_block: Some(0),
                dao_fork_block: Some(0),
                dao_fork_support: false,
                eip150_block: Some(0),
                eip155_block: Some(0),
                eip158_block: Some(0),
                byzantium_block: Some(0),
                constantinople_block: Some(0),
                petersburg_block: Some(0),
                istanbul_block: Some(0),
                muir_glacier_block: Some(0),
                berlin_block: Some(0),
                london_block: Some(0),
                arrow_glacier_block: Some(0),
                gray_glacier_block: Some(0),
                merge_netsplit_block: Some(0),
                shanghai_time: Some(0),
                cancun_time: Some(0),
                terminal_total_difficulty: Some(U256::ZERO),
                ..Default::default()
            },
            ..Default::default()
        };

        let chain_spec: ChainSpec = genesis.into();

        let hardforks: Vec<_> = chain_spec.hardforks.forks_iter().map(|(h, _)| h).collect();
        let expected_hardforks = vec![
            EthereumHardfork::Homestead.boxed(),
            EthereumHardfork::Dao.boxed(),
            EthereumHardfork::Tangerine.boxed(),
            EthereumHardfork::SpuriousDragon.boxed(),
            EthereumHardfork::Byzantium.boxed(),
            EthereumHardfork::Constantinople.boxed(),
            EthereumHardfork::Petersburg.boxed(),
            EthereumHardfork::Istanbul.boxed(),
            EthereumHardfork::MuirGlacier.boxed(),
            EthereumHardfork::Berlin.boxed(),
            EthereumHardfork::London.boxed(),
            EthereumHardfork::ArrowGlacier.boxed(),
            EthereumHardfork::GrayGlacier.boxed(),
            EthereumHardfork::Paris.boxed(),
            EthereumHardfork::Shanghai.boxed(),
            EthereumHardfork::Cancun.boxed(),
        ];

        assert!(expected_hardforks
            .iter()
            .zip(hardforks.iter())
            .all(|(expected, actual)| &**expected == *actual));
        assert_eq!(expected_hardforks.len(), hardforks.len());
    }
}
