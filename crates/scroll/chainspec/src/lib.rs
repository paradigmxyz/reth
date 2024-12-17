//! Scroll-Reth chain specs.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

use alloc::{boxed::Box, vec::Vec};
use alloy_chains::Chain;
use alloy_consensus::Header;
use alloy_genesis::Genesis;
use alloy_primitives::{B256, U256};
use derive_more::{Constructor, Deref, Display, From, Into};
use reth_chainspec::{
    BaseFeeParams, ChainSpec, ChainSpecBuilder, DepositContract, EthChainSpec, EthereumHardforks,
    ForkFilter, ForkId, Hardforks, Head,
};
use reth_ethereum_forks::{ChainHardforks, EthereumHardfork, ForkCondition, Hardfork};
use reth_network_peers::NodeRecord;
use reth_scroll_forks::ScrollHardforks;

#[cfg(not(feature = "std"))]
use once_cell::sync::Lazy as LazyLock;
#[cfg(feature = "std")]
use std::sync::LazyLock;

extern crate alloc;

mod constants;
pub use constants::{
    SCROLL_DEV_L1_CONFIG, SCROLL_DEV_L1_MESSAGE_QUEUE_ADDRESS, SCROLL_DEV_L1_PROXY_ADDRESS,
    SCROLL_DEV_MAX_L1_MESSAGES, SCROLL_FEE_VAULT_ADDRESS, SCROLL_MAINNET_L1_CONFIG,
    SCROLL_MAINNET_L1_MESSAGE_QUEUE_ADDRESS, SCROLL_MAINNET_L1_PROXY_ADDRESS,
    SCROLL_MAINNET_MAX_L1_MESSAGES, SCROLL_SEPOLIA_L1_CONFIG,
    SCROLL_SEPOLIA_L1_MESSAGE_QUEUE_ADDRESS, SCROLL_SEPOLIA_L1_PROXY_ADDRESS,
    SCROLL_SEPOLIA_MAX_L1_MESSAGES,
};

mod dev;
pub use dev::SCROLL_DEV;

mod genesis;
pub use genesis::{ScrollChainConfig, ScrollChainInfo};

// convenience re-export of the chain spec provider.
pub use reth_chainspec::ChainSpecProvider;

mod scroll;
pub use scroll::SCROLL_MAINNET;

mod scroll_sepolia;
pub use scroll_sepolia::SCROLL_SEPOLIA;

/// Chain spec builder for a Scroll chain.
#[derive(Debug, Default, From)]
pub struct ScrollChainSpecBuilder {
    /// [`ChainSpecBuilder`]
    inner: ChainSpecBuilder,
}

impl ScrollChainSpecBuilder {
    /// Construct a new builder from the scroll mainnet chain spec.
    pub fn scroll_mainnet() -> Self {
        Self {
            inner: ChainSpecBuilder::default()
                .chain(SCROLL_MAINNET.chain)
                .genesis(SCROLL_MAINNET.genesis.clone())
                .with_forks(SCROLL_MAINNET.hardforks.clone()),
        }
    }

    /// Construct a new builder from the scroll sepolia chain spec.
    pub fn scroll_sepolia() -> Self {
        Self {
            inner: ChainSpecBuilder::default()
                .chain(SCROLL_SEPOLIA.chain)
                .genesis(SCROLL_SEPOLIA.genesis.clone())
                .with_forks(SCROLL_SEPOLIA.hardforks.clone()),
        }
    }
}

impl ScrollChainSpecBuilder {
    /// Set the chain ID
    pub fn chain(mut self, chain: Chain) -> Self {
        self.inner = self.inner.chain(chain);
        self
    }

    /// Set the genesis block.
    pub fn genesis(mut self, genesis: Genesis) -> Self {
        self.inner = self.inner.genesis(genesis);
        self
    }

    /// Add the given fork with the given activation condition to the spec.
    pub fn with_fork<H: Hardfork>(mut self, fork: H, condition: ForkCondition) -> Self {
        self.inner = self.inner.with_fork(fork, condition);
        self
    }

    /// Add the given forks with the given activation condition to the spec.
    pub fn with_forks(mut self, forks: ChainHardforks) -> Self {
        self.inner = self.inner.with_forks(forks);
        self
    }

    /// Remove the given fork from the spec.
    pub fn without_fork(mut self, fork: reth_scroll_forks::ScrollHardfork) -> Self {
        self.inner = self.inner.without_fork(fork);
        self
    }

    /// Enable Archimedes at genesis
    pub fn archimedes_activated(mut self) -> Self {
        self.inner = self.inner.london_activated();
        self.inner = self
            .inner
            .with_fork(reth_scroll_forks::ScrollHardfork::Archimedes, ForkCondition::Block(0));
        self
    }

    /// Enable Bernoulli at genesis
    pub fn bernoulli_activated(mut self) -> Self {
        self = self.archimedes_activated();
        self.inner = self.inner.with_fork(EthereumHardfork::Shanghai, ForkCondition::Timestamp(0));
        self.inner = self
            .inner
            .with_fork(reth_scroll_forks::ScrollHardfork::Bernoulli, ForkCondition::Block(0));
        self
    }

    /// Enable Curie at genesis
    pub fn curie_activated(mut self) -> Self {
        self = self.bernoulli_activated();
        self.inner =
            self.inner.with_fork(reth_scroll_forks::ScrollHardfork::Curie, ForkCondition::Block(0));
        self
    }

    /// Enable Darwin at genesis
    pub fn darwin_activated(mut self) -> Self {
        self = self.curie_activated();
        self.inner = self
            .inner
            .with_fork(reth_scroll_forks::ScrollHardfork::Darwin, ForkCondition::Timestamp(0));
        self
    }

    /// Enable `DarwinV2` at genesis
    pub fn darwin_v2_activated(mut self) -> Self {
        self = self.darwin_activated();
        self.inner = self
            .inner
            .with_fork(reth_scroll_forks::ScrollHardfork::DarwinV2, ForkCondition::Timestamp(0));
        self
    }

    /// Build the resulting [`ScrollChainSpec`].
    ///
    /// # Panics
    ///
    /// This function panics if the chain ID and genesis is not set ([`Self::chain`] and
    /// [`Self::genesis`])
    pub fn build(self, config: ScrollChainConfig) -> ScrollChainSpec {
        ScrollChainSpec { inner: self.inner.build(), config }
    }
}

/// Scroll chain spec type.
#[derive(Debug, Clone, Deref, Into, Constructor, PartialEq, Eq)]
pub struct ScrollChainSpec {
    /// [`ChainSpec`].
    #[deref]
    pub inner: ChainSpec,
    /// [`ScrollChainConfig`]
    pub config: ScrollChainConfig,
}

impl EthChainSpec for ScrollChainSpec {
    fn chain(&self) -> alloy_chains::Chain {
        self.inner.chain()
    }

    fn base_fee_params_at_block(&self, block_number: u64) -> BaseFeeParams {
        // TODO(scroll): need to implement Scroll L2 formula related to https://github.com/scroll-tech/reth/issues/60
        self.inner.base_fee_params_at_block(block_number)
    }

    fn base_fee_params_at_timestamp(&self, timestamp: u64) -> BaseFeeParams {
        // TODO(scroll): need to implement Scroll L2 formula related to https://github.com/scroll-tech/reth/issues/60
        self.inner.base_fee_params_at_timestamp(timestamp)
    }

    fn deposit_contract(&self) -> Option<&DepositContract> {
        self.inner.deposit_contract()
    }

    fn genesis_hash(&self) -> B256 {
        *self.genesis_hash.get_or_init(|| self.genesis_header().hash_slow())
    }

    fn prune_delete_limit(&self) -> usize {
        self.inner.prune_delete_limit()
    }

    fn display_hardforks(&self) -> Box<dyn Display> {
        Box::new(ChainSpec::display_hardforks(self))
    }

    fn genesis_header(&self) -> &Header {
        self.inner.genesis_header.get_or_init(|| self.make_genesis_header())
    }

    fn genesis(&self) -> &Genesis {
        self.inner.genesis()
    }

    fn max_gas_limit(&self) -> u64 {
        self.inner.max_gas_limit()
    }

    fn bootnodes(&self) -> Option<Vec<NodeRecord>> {
        self.inner.bootnodes()
    }
}

impl ScrollChainSpec {
    fn make_genesis_header(&self) -> Header {
        Header {
            gas_limit: self.genesis.gas_limit,
            difficulty: self.genesis.difficulty,
            nonce: self.genesis.nonce.into(),
            extra_data: self.genesis.extra_data.clone(),
            state_root: reth_scroll_state_commitment::state_root_ref_unhashed(&self.genesis.alloc),
            timestamp: self.genesis.timestamp,
            mix_hash: self.genesis.mix_hash,
            beneficiary: self.genesis.coinbase,
            base_fee_per_gas: None,
            withdrawals_root: None,
            parent_beacon_block_root: None,
            blob_gas_used: None,
            excess_blob_gas: None,
            requests_hash: None,
            ..Default::default()
        }
    }
}

impl Hardforks for ScrollChainSpec {
    fn fork<H: reth_chainspec::Hardfork>(&self, fork: H) -> reth_chainspec::ForkCondition {
        self.inner.fork(fork)
    }

    fn forks_iter(
        &self,
    ) -> impl Iterator<Item = (&dyn reth_chainspec::Hardfork, reth_chainspec::ForkCondition)> {
        self.inner.forks_iter()
    }

    fn fork_id(&self, head: &Head) -> ForkId {
        self.inner.fork_id(head)
    }

    fn latest_fork_id(&self) -> ForkId {
        self.inner.latest_fork_id()
    }

    fn fork_filter(&self, head: Head) -> ForkFilter {
        self.inner.fork_filter(head)
    }
}

impl EthereumHardforks for ScrollChainSpec {
    fn get_final_paris_total_difficulty(&self) -> Option<U256> {
        self.inner.get_final_paris_total_difficulty()
    }

    fn final_paris_total_difficulty(&self, block_number: u64) -> Option<U256> {
        self.inner.final_paris_total_difficulty(block_number)
    }
}

impl ScrollHardforks for ScrollChainSpec {}

impl From<Genesis> for ScrollChainSpec {
    fn from(genesis: Genesis) -> Self {
        use reth_scroll_forks::ScrollHardfork;
        let scroll_chain_info = ScrollConfigInfo::extract_from(&genesis);
        let hard_fork_info =
            scroll_chain_info.scroll_chain_info.hard_fork_info.expect("load scroll hard fork info");

        // Block-based hardforks
        let hardfork_opts = [
            (EthereumHardfork::Homestead.boxed(), genesis.config.homestead_block),
            (EthereumHardfork::Tangerine.boxed(), genesis.config.eip150_block),
            (EthereumHardfork::SpuriousDragon.boxed(), genesis.config.eip155_block),
            (EthereumHardfork::Byzantium.boxed(), genesis.config.byzantium_block),
            (EthereumHardfork::Constantinople.boxed(), genesis.config.constantinople_block),
            (EthereumHardfork::Petersburg.boxed(), genesis.config.petersburg_block),
            (EthereumHardfork::Istanbul.boxed(), genesis.config.istanbul_block),
            (EthereumHardfork::Berlin.boxed(), genesis.config.berlin_block),
            (EthereumHardfork::London.boxed(), genesis.config.london_block),
            (ScrollHardfork::Archimedes.boxed(), hard_fork_info.archimedes_block),
            (ScrollHardfork::Bernoulli.boxed(), hard_fork_info.bernoulli_block),
            (ScrollHardfork::Curie.boxed(), hard_fork_info.curie_block),
        ];
        let mut block_hardforks = hardfork_opts
            .into_iter()
            .filter_map(|(hardfork, opt)| opt.map(|block| (hardfork, ForkCondition::Block(block))))
            .collect::<Vec<_>>();

        // Time-based hardforks
        let time_hardfork_opts = [
            (EthereumHardfork::Shanghai.boxed(), genesis.config.shanghai_time),
            (ScrollHardfork::Darwin.boxed(), hard_fork_info.darwin_time),
            (ScrollHardfork::DarwinV2.boxed(), hard_fork_info.darwin_v2_time),
        ];

        let mut time_hardforks = time_hardfork_opts
            .into_iter()
            .filter_map(|(hardfork, opt)| {
                opt.map(|time| (hardfork, ForkCondition::Timestamp(time)))
            })
            .collect::<Vec<_>>();

        block_hardforks.append(&mut time_hardforks);

        // Ordered Hardforks
        let mainnet_hardforks = ScrollHardfork::scroll_mainnet();
        let mainnet_order = mainnet_hardforks.forks_iter();

        let mut ordered_hardforks = Vec::with_capacity(block_hardforks.len());
        for (hardfork, _) in mainnet_order {
            if let Some(pos) = block_hardforks.iter().position(|(e, _)| **e == *hardfork) {
                ordered_hardforks.push(block_hardforks.remove(pos));
            }
        }

        // append the remaining unknown hardforks to ensure we don't filter any out
        ordered_hardforks.append(&mut block_hardforks);

        Self {
            inner: ChainSpec {
                chain: genesis.config.chain_id.into(),
                genesis,
                hardforks: ChainHardforks::new(ordered_hardforks),
                ..Default::default()
            },
            config: scroll_chain_info.scroll_chain_info.scroll_chain_config,
        }
    }
}

#[derive(Default, Debug)]
struct ScrollConfigInfo {
    scroll_chain_info: ScrollChainInfo,
}

impl ScrollConfigInfo {
    fn extract_from(genesis: &Genesis) -> Self {
        Self {
            scroll_chain_info: ScrollChainInfo::extract_from(&genesis.config.extra_fields)
                .expect("extract scroll extra fields failed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::*;
    use alloy_genesis::{ChainConfig, Genesis};
    use alloy_primitives::b256;
    use reth_chainspec::test_fork_ids;
    use reth_ethereum_forks::{EthereumHardfork, ForkHash};
    use reth_scroll_forks::ScrollHardfork;

    #[test]
    fn scroll_mainnet_genesis_hash() {
        let scroll_mainnet =
            ScrollChainSpecBuilder::scroll_mainnet().build(ScrollChainConfig::mainnet());
        assert_eq!(
            b256!("bbc05efd412b7cd47a2ed0e5ddfcf87af251e414ea4c801d78b6784513180a80"),
            scroll_mainnet.genesis_hash()
        );
    }

    #[test]
    fn scroll_sepolia_genesis_hash() {
        let scroll_sepolia =
            ScrollChainSpecBuilder::scroll_sepolia().build(ScrollChainConfig::sepolia());
        assert_eq!(
            b256!("aa62d1a8b2bffa9e5d2368b63aae0d98d54928bd713125e3fd9e5c896c68592c"),
            scroll_sepolia.genesis_hash()
        );
    }

    #[test]
    fn scroll_mainnet_forkids() {
        test_fork_ids(
            &SCROLL_MAINNET,
            &[
                (
                    Head { number: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0xea, 0x6b, 0x56, 0xca]), next: 5220340 },
                ),
                (
                    Head { number: 5220340, ..Default::default() },
                    ForkId { hash: ForkHash([0xee, 0x46, 0xae, 0x2a]), next: 7096836 },
                ),
                (
                    Head { number: 7096836, ..Default::default() },
                    ForkId { hash: ForkHash([0x18, 0xd3, 0xc8, 0xd9]), next: 1724227200 },
                ),
                (
                    Head { number: 7096836, timestamp: 1724227200, ..Default::default() },
                    ForkId { hash: ForkHash([0xcc, 0xeb, 0x09, 0xb0]), next: 1725264000 },
                ),
                (
                    Head { number: 7096836, timestamp: 1725264000, ..Default::default() },
                    ForkId { hash: ForkHash([0x21, 0xa2, 0x07, 0x54]), next: 0 },
                ),
            ],
        );
    }

    #[test]
    fn scroll_sepolia_forkids() {
        test_fork_ids(
            &SCROLL_SEPOLIA,
            &[
                (
                    Head { number: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0x25, 0xfa, 0xe4, 0x54]), next: 3747132 },
                ),
                (
                    Head { number: 3747132, ..Default::default() },
                    ForkId { hash: ForkHash([0xda, 0x76, 0xc2, 0x2d]), next: 4740239 },
                ),
                (
                    Head { number: 4740239, ..Default::default() },
                    ForkId { hash: ForkHash([0x9f, 0xb4, 0x75, 0xf1]), next: 1723622400 },
                ),
                (
                    Head { number: 4740239, timestamp: 1723622400, ..Default::default() },
                    ForkId { hash: ForkHash([0xe9, 0x26, 0xd4, 0x9b]), next: 1724832000 },
                ),
                (
                    Head { number: 4740239, timestamp: 1724832000, ..Default::default() },
                    ForkId { hash: ForkHash([0x69, 0xf3, 0x7e, 0xde]), next: 0 },
                ),
            ],
        );
    }

    #[test]
    fn is_bernoulli_active() {
        let scroll_mainnet =
            ScrollChainSpecBuilder::scroll_mainnet().build(ScrollChainConfig::mainnet());
        assert!(!scroll_mainnet.is_bernoulli_active_at_block(1))
    }

    #[test]
    fn parse_scroll_hardforks() {
        let geth_genesis = r#"
    {
      "config": {
        "bernoulliBlock": 10,
        "curieBlock": 20,
        "darwinTime": 30,
        "darwinV2Time": 31,
        "scroll": {
            "feeVaultAddress": "0x5300000000000000000000000000000000000005",
            "l1Config": {
                "l1ChainId": 1,
                "l1MessageQueueAddress": "0x0d7E906BD9cAFa154b048cFa766Cc1E54E39AF9B",
                "scrollChainAddress": "0xa13BAF47339d63B743e7Da8741db5456DAc1E556",
                "numL1MessagesPerBlock": 10
            }
        }
      }
    }
    "#;
        let genesis: Genesis = serde_json::from_str(geth_genesis).unwrap();

        let actual_bernoulli_block = genesis.config.extra_fields.get("bernoulliBlock");
        assert_eq!(actual_bernoulli_block, Some(serde_json::Value::from(10)).as_ref());
        let actual_curie_block = genesis.config.extra_fields.get("curieBlock");
        assert_eq!(actual_curie_block, Some(serde_json::Value::from(20)).as_ref());
        let actual_darwin_timestamp = genesis.config.extra_fields.get("darwinTime");
        assert_eq!(actual_darwin_timestamp, Some(serde_json::Value::from(30)).as_ref());
        let actual_darwin_v2_timestamp = genesis.config.extra_fields.get("darwinV2Time");
        assert_eq!(actual_darwin_v2_timestamp, Some(serde_json::Value::from(31)).as_ref());
        let scroll_object = genesis.config.extra_fields.get("scroll").unwrap();
        assert_eq!(
            scroll_object,
            &serde_json::json!({
                "feeVaultAddress": "0x5300000000000000000000000000000000000005",
                "l1Config": {
                    "l1ChainId": 1,
                    "l1MessageQueueAddress": "0x0d7E906BD9cAFa154b048cFa766Cc1E54E39AF9B",
                    "scrollChainAddress": "0xa13BAF47339d63B743e7Da8741db5456DAc1E556",
                    "numL1MessagesPerBlock": 10
                }
            })
        );

        let chain_spec: ScrollChainSpec = genesis.into();

        assert!(!chain_spec.is_fork_active_at_block(ScrollHardfork::Bernoulli, 0));
        assert!(!chain_spec.is_fork_active_at_block(ScrollHardfork::Curie, 0));
        assert!(!chain_spec.is_fork_active_at_timestamp(ScrollHardfork::Darwin, 0));
        assert!(!chain_spec.is_fork_active_at_timestamp(ScrollHardfork::DarwinV2, 0));

        assert!(chain_spec.is_fork_active_at_block(ScrollHardfork::Bernoulli, 10));
        assert!(chain_spec.is_fork_active_at_block(ScrollHardfork::Curie, 20));
        assert!(chain_spec.is_fork_active_at_timestamp(ScrollHardfork::Darwin, 30));
        assert!(chain_spec.is_fork_active_at_timestamp(ScrollHardfork::DarwinV2, 31));
    }

    #[test]
    fn test_fork_order_scroll_mainnet() {
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
                berlin_block: Some(0),
                london_block: Some(0),
                shanghai_time: Some(0),
                extra_fields: [
                    (String::from("archimedesBlock"), 0.into()),
                    (String::from("bernoulliBlock"), 0.into()),
                    (String::from("curieBlock"), 0.into()),
                    (String::from("darwinTime"), 0.into()),
                    (String::from("darwinV2Time"), 0.into()),
                    (
                        String::from("scroll"),
                        serde_json::json!({
                            "feeVaultAddress": "0x5300000000000000000000000000000000000005",
                            "l1Config": {
                                "l1ChainId": 1,
                                "l1MessageQueueAddress": "0x0d7E906BD9cAFa154b048cFa766Cc1E54E39AF9B",
                                "scrollChainAddress": "0xa13BAF47339d63B743e7Da8741db5456DAc1E556",
                                "numL1MessagesPerBlock": 10
                            }
                        }),
                    ),
                ]
                .into_iter()
                .collect(),
                ..Default::default()
            },
            ..Default::default()
        };

        let chain_spec: ScrollChainSpec = genesis.into();

        let hardforks: Vec<_> = chain_spec.hardforks.forks_iter().map(|(h, _)| h).collect();
        let expected_hardforks = vec![
            EthereumHardfork::Homestead.boxed(),
            EthereumHardfork::Tangerine.boxed(),
            EthereumHardfork::SpuriousDragon.boxed(),
            EthereumHardfork::Byzantium.boxed(),
            EthereumHardfork::Constantinople.boxed(),
            EthereumHardfork::Petersburg.boxed(),
            EthereumHardfork::Istanbul.boxed(),
            EthereumHardfork::Berlin.boxed(),
            EthereumHardfork::London.boxed(),
            ScrollHardfork::Archimedes.boxed(),
            EthereumHardfork::Shanghai.boxed(),
            ScrollHardfork::Bernoulli.boxed(),
            ScrollHardfork::Curie.boxed(),
            ScrollHardfork::Darwin.boxed(),
            ScrollHardfork::DarwinV2.boxed(),
        ];

        assert!(expected_hardforks
            .iter()
            .zip(hardforks.iter())
            .all(|(expected, actual)| &**expected == *actual));

        assert_eq!(expected_hardforks.len(), hardforks.len());
    }
}
