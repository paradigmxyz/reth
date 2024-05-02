use alloy_genesis::Genesis;
use reth_ethereum_forks::{ForkFilter, ForkFilterKey, ForkHash, ForkId};
use serde::{Deserialize, Serialize};

use crate::{
    constants::{EIP1559_INITIAL_BASE_FEE, EMPTY_RECEIPTS, EMPTY_TRANSACTIONS, EMPTY_WITHDRAWALS},
    holesky_nodes,
    net::{goerli_nodes, mainnet_nodes, sepolia_nodes},
    proofs::state_root_ref_unhashed,
    revm_primitives::{b256, B256},
    Chain, ChainKind, Hardfork, Head, Header, NamedChain, NodeRecord, SealedHeader, U256,
};

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fmt::{Display, Formatter},
    sync::Arc,
};

pub use alloy_eips::eip1559::BaseFeeParams;

// ... (rest of the code)
/// An Ethereum chain specification.
///
/// A chain specification describes:
///
/// - Meta-information about the chain (the chain ID)
/// - The genesis block of the chain ([`Genesis`])
/// - What hardforks are activated, and under which conditions

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ChainSpec {
    /// The chain ID
    pub chain: Chain,

    /// The hash of the genesis block.
    ///
    /// This acts as a small cache for known chains. If the chain is known, then the genesis hash
    /// is also known ahead of time, and this will be `Some`.
    #[serde(skip, default)]
    pub genesis_hash: Option<B256>,

    /// The genesis block
    pub genesis: Genesis,

    /// The block at which [Hardfork::Paris] was activated and the final difficulty at this block.
    #[serde(skip, default)]
    pub paris_block_and_final_difficulty: Option<(u64, U256)>,

    /// The active hard forks and their activation conditions
    pub hardforks: BTreeMap<Hardfork, ForkCondition>,

    /// The deposit contract deployed for PoS
    #[serde(skip, default)]
    pub deposit_contract: Option<DepositContract>,

    /// The parameters that configure how a block's base fee is computed
    pub base_fee_params: BaseFeeParamsKind,

    /// The delete limit for pruner, per block. In the actual pruner run it will be multiplied by
    /// the amount of blocks between pruner runs to account for the difference in amount of new
    /// data coming in.
    #[serde(default)]
    pub prune_delete_limit: usize,
}

impl Default for ChainSpec {
    fn default() -> ChainSpec {
        ChainSpec {
            chain: Default::default(),
            genesis_hash: Default::default(),
            genesis: Default::default(),
            paris_block_and_final_difficulty: Default::default(),
            hardforks: Default::default(),
            deposit_contract: Default::default(),
            base_fee_params: BaseFeeParamsKind::Constant(BaseFeeParams::ethereum()),
            prune_delete_limit: MAINNET.prune_delete_limit,
        }
    }
}

impl ChainSpec {
    /// Get information about the chain itself
    pub fn chain(&self) -> Chain {
        self.chain
    }

    /// Returns `true` if this chain contains Ethereum configuration.
    #[inline]
    pub fn is_eth(&self) -> bool {
        matches!(
            self.chain.kind(),
            ChainKind::Named(
                NamedChain::Mainnet |
                    NamedChain::Morden |
                    NamedChain::Ropsten |
                    NamedChain::Rinkeby |
                    NamedChain::Goerli |
                    NamedChain::Kovan |
                    NamedChain::Holesky |
                    NamedChain::Sepolia
            )
        )
    }

    /// Returns `true` if this chain contains Optimism configuration.
    #[inline]
    pub fn is_optimism(&self) -> bool {
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
    pub fn genesis(&self) -> &Genesis {
        &self.genesis
    }

    /// Get the header for the genesis block.
    pub fn genesis_header(&self) -> Header {
        // If London is activated at genesis, we set the initial base fee as per EIP-1559.
        let base_fee_per_gas = self.initial_base_fee();

        // If shanghai is activated, initialize the header with an empty withdrawals hash, and
        // empty withdrawals list.
        let withdrawals_root = self
            .fork(Hardfork::Shanghai)
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

        Header {
            parent_hash: B256::ZERO,
            number: 0,
            transactions_root: EMPTY_TRANSACTIONS,
            ommers_hash: EMPTY_OMMER_ROOT_HASH,
            receipts_root: EMPTY_RECEIPTS,
            logs_bloom: Default::default(),
            gas_limit: self.genesis.gas_limit as u64,
            difficulty: self.genesis.difficulty,
            nonce: self.genesis.nonce,
            extra_data: self.genesis.extra_data.clone(),
            state_root: state_root_ref_unhashed(&self.genesis.alloc),
            timestamp: self.genesis.timestamp,
            mix_hash: self.genesis.mix_hash,
            beneficiary: self.genesis.coinbase,
            gas_used: Default::default(),
            base_fee_per_gas,
            withdrawals_root,
            parent_beacon_block_root,
            blob_gas_used,
            excess_blob_gas,
        }
    }

    /// Get the sealed header for the genesis block.
    pub fn sealed_genesis_header(&self) -> SealedHeader {
        SealedHeader::new(self.genesis_header(), self.genesis_hash())
    }

    /// Get the initial base fee of the genesis block.
    pub fn initial_base_fee(&self) -> Option<u64> {
        // If the base fee is set in the genesis block, we use that instead of the default.
        let genesis_base_fee =
            self.genesis.base_fee_per_gas.map(|fee| fee as u64).unwrap_or(EIP1559_INITIAL_BASE_FEE);

        // If London is activated at genesis, we set the initial base fee as per EIP-1559.
        self.fork(Hardfork::London).active_at_block(0).then_some(genesis_base_fee)
    }

    /// Get the [BaseFeeParams] for the chain at the given timestamp.
    pub fn base_fee_params_at_timestamp(&self, timestamp: u64) -> BaseFeeParams {
        match self.base_fee_params {
            BaseFeeParamsKind::Constant(bf_params) => bf_params,
            BaseFeeParamsKind::Variable(ForkBaseFeeParams(ref bf_params)) => {
                // Walk through the base fee params configuration in reverse order, and return the
                // first one that corresponds to a hardfork that is active at the
                // given timestamp.
                for (fork, params) in bf_params.iter().rev() {
                    if self.is_fork_active_at_timestamp(*fork, timestamp) {
                        return *params
                    }
                }

                bf_params.first().map(|(_, params)| *params).unwrap_or(BaseFeeParams::ethereum())
            }
        }
    }

    /// Get the [BaseFeeParams] for the chain at the given block number
    pub fn base_fee_params_at_block(&self, block_number: u64) -> BaseFeeParams {
        match self.base_fee_params {
            BaseFeeParamsKind::Constant(bf_params) => bf_params,
            BaseFeeParamsKind::Variable(ForkBaseFeeParams(ref bf_params)) => {
                // Walk through the base fee params configuration in reverse order, and return the
                // first one that corresponds to a hardfork that is active at the
                // given timestamp.
                for (fork, params) in bf_params.iter().rev() {
                    if self.is_fork_active_at_block(*fork, block_number) {
                        return *params
                    }
                }

                bf_params.first().map(|(_, params)| *params).unwrap_or(BaseFeeParams::ethereum())
            }
        }
    }

    /// Get the hash of the genesis block.
    pub fn genesis_hash(&self) -> B256 {
        self.genesis_hash.unwrap_or_else(|| self.genesis_header().hash_slow())
    }

    /// Get the timestamp of the genesis block.
    pub fn genesis_timestamp(&self) -> u64 {
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
    pub fn hardfork_fork_filter(&self, fork: Hardfork) -> Option<ForkFilter> {
        match self.fork(fork) {
            ForkCondition::Never => None,
            _ => Some(self.fork_filter(self.satisfy(self.fork(fork)))),
        }
    }

    /// Returns the forks in this specification and their activation conditions.
    pub fn hardforks(&self) -> &BTreeMap<Hardfork, ForkCondition> {
        &self.hardforks
    }

    /// Returns the hardfork display helper.
    pub fn display_hardforks(&self) -> DisplayHardforks {
        DisplayHardforks::new(
            self.hardforks(),
            self.paris_block_and_final_difficulty.map(|(block, _)| block),
        )
    }

    /// Get the fork id for the given hardfork.
    #[inline]
    pub fn hardfork_fork_id(&self, fork: Hardfork) -> Option<ForkId> {
        match self.fork(fork) {
            ForkCondition::Never => None,
            _ => Some(self.fork_id(&self.satisfy(self.fork(fork)))),
        }
    }

    /// Convenience method to get the fork id for [Hardfork::Shanghai] from a given chainspec.
    #[inline]
    pub fn shanghai_fork_id(&self) -> Option<ForkId> {
        self.hardfork_fork_id(Hardfork::Shanghai)
    }

    /// Convenience method to get the fork id for [Hardfork::Cancun] from a given chainspec.
    #[inline]
    pub fn cancun_fork_id(&self) -> Option<ForkId> {
        self.hardfork_fork_id(Hardfork::Cancun)
    }

    /// Convenience method to get the latest fork id from the chainspec. Panics if chainspec has no
    /// hardforks.
    #[inline]
    pub fn latest_fork_id(&self) -> ForkId {
        self.hardfork_fork_id(*self.hardforks().last_key_value().unwrap().0).unwrap()
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

    /// Convenience method to check if a fork is active at a given block number
    #[inline]
    pub fn is_fork_active_at_block(&self, fork: Hardfork, block_number: u64) -> bool {
        self.fork(fork).active_at_block(block_number)
    }

    /// Convenience method to check if [Hardfork::Shanghai] is active at a given timestamp.
    #[inline]
    pub fn is_shanghai_active_at_timestamp(&self, timestamp: u64) -> bool {
        self.is_fork_active_at_timestamp(Hardfork::Shanghai, timestamp)
    }

    /// Convenience method to check if [Hardfork::Cancun] is active at a given timestamp.
    #[inline]
    pub fn is_cancun_active_at_timestamp(&self, timestamp: u64) -> bool {
        self.is_fork_active_at_timestamp(Hardfork::Cancun, timestamp)
    }

    /// Convenience method to check if [Hardfork::Prague] is active at a given timestamp.
    #[inline]
    pub fn is_prague_active_at_timestamp(&self, timestamp: u64) -> bool {
        self.is_fork_active_at_timestamp(Hardfork::Prague, timestamp)
    }

    /// Convenience method to check if [Hardfork::Byzantium] is active at a given block number.
    #[inline]
    pub fn is_byzantium_active_at_block(&self, block_number: u64) -> bool {
        self.fork(Hardfork::Byzantium).active_at_block(block_number)
    }

    /// Convenience method to check if [Hardfork::SpuriousDragon] is active at a given block number.
    #[inline]
    pub fn is_spurious_dragon_active_at_block(&self, block_number: u64) -> bool {
        self.fork(Hardfork::SpuriousDragon).active_at_block(block_number)
    }

    /// Convenience method to check if [Hardfork::Homestead] is active at a given block number.
    #[inline]
    pub fn is_homestead_active_at_block(&self, block_number: u64) -> bool {
        self.fork(Hardfork::Homestead).active_at_block(block_number)
    }

    /// Convenience method to check if [Hardfork::Bedrock] is active at a given block number.
    #[cfg(feature = "optimism")]
    #[inline]
    pub fn is_bedrock_active_at_block(&self, block_number: u64) -> bool {
        self.fork(Hardfork::Bedrock).active_at_block(block_number)
    }

    /// Creates a [`ForkFilter`] for the block described by [Head].
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

        ForkFilter::new(head, self.genesis_hash(), self.genesis_timestamp(), forks)
    }

    /// Compute the [`ForkId`] for the given [`Head`] following eip-6122 spec
    pub fn fork_id(&self, head: &Head) -> ForkId {
        let mut forkhash = ForkHash::from(self.genesis_hash());
        let mut current_applied = 0;

        // handle all block forks before handling timestamp based forks. see: https://eips.ethereum.org/EIPS/eip-6122
        for (_, cond) in self.forks_iter() {
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
        for timestamp in self.forks_iter().filter_map(|(_, cond)| {
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
                if let Some(last_block_num) = self.last_block_fork_before_merge_or_timestamp() {
                    return Head { timestamp, number: last_block_num, ..Default::default() }
                }
                Head { timestamp, ..Default::default() }
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
    /// Note: this returns None if the ChainSpec is not configured with a TTD/Timestamp fork.
    pub(crate) fn last_block_fork_before_merge_or_timestamp(&self) -> Option<u64> {
        let mut hardforks_iter = self.forks_iter().peekable();
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
            C::Goerli => Some(goerli_nodes()),
            C::Sepolia => Some(sepolia_nodes()),
            C::Holesky => Some(holesky_nodes()),
            #[cfg(feature = "optimism")]
            C::Base => Some(base_nodes()),
            #[cfg(feature = "optimism")]
            C::Optimism => Some(op_nodes()),
            #[cfg(feature = "optimism")]
            C::BaseGoerli | C::BaseSepolia => Some(base_testnet_nodes()),
            #[cfg(feature = "optimism")]
            C::OptimismSepolia | C::OptimismGoerli | C::OptimismKovan => Some(op_testnet_nodes()),
            _ => None,
        }
    }
}

impl From<Genesis> for ChainSpec {
    fn from(genesis: Genesis) -> Self {
        #[cfg(feature = "optimism")]
        let optimism_genesis_info = OptimismGenesisInfo::extract_from(&genesis);

        // Block-based hardforks
        let hardfork_opts = [
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
            #[cfg(feature = "optimism")]
            (Hardfork::Bedrock, optimism_genesis_info.bedrock_block),
        ];
        let mut hardforks = hardfork_opts
            .iter()
            .filter_map(|(hardfork, opt)| opt.map(|block| (*hardfork, ForkCondition::Block(block))))
            .collect::<BTreeMap<_, _>>();

        // Paris
        let paris_block_and_final_difficulty =
            if let Some(ttd) = genesis.config.terminal_total_difficulty {
                hardforks.insert(
                    Hardfork::Paris,
                    ForkCondition::TTD {
                        total_difficulty: ttd,
                        fork_block: genesis.config.merge_netsplit_block,
                    },
                );

                genesis.config.merge_netsplit_block.map(|block| (block, ttd))
            } else {
                None
            };

        // Time-based hardforks
        let time_hardfork_opts = [
            (Hardfork::Shanghai, genesis.config.shanghai_time),
            (Hardfork::Cancun, genesis.config.cancun_time),
            #[cfg(feature = "optimism")]
            (Hardfork::Regolith, optimism_genesis_info.regolith_time),
            #[cfg(feature = "optimism")]
            (Hardfork::Ecotone, optimism_genesis_info.ecotone_time),
            #[cfg(feature = "optimism")]
            (Hardfork::Canyon, optimism_genesis_info.canyon_time),
        ];

        let time_hardforks = time_hardfork_opts
            .iter()
            .filter_map(|(hardfork, opt)| {
                opt.map(|time| (*hardfork, ForkCondition::Timestamp(time)))
            })
            .collect::<BTreeMap<_, _>>();

        hardforks.extend(time_hardforks);

        Self {
            chain: genesis.config.chain_id.into(),
            genesis,
            genesis_hash: None,
            hardforks,
            paris_block_and_final_difficulty,
            deposit_contract: None,
            ..Default::default()
        }
    }
}
