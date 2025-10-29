//! Custom chain specification integrating hardforks.
//!
//! This demonstrates how to build a `ChainSpec` with custom hardforks,
//! implementing required traits for integration with Reth's chain management.

use crate::hardforks::CustomHardforkConfig;
use alloy_genesis::Genesis;
use reth_chainspec::{Chain, ChainSpec, EthChainSpec, EthereumHardforks, ForkCondition, Hardfork, Hardforks};
use reth_ethereum::chainspec::EthereumHardfork;
use reth_network_peers::NodeRecord;

// Custom chain spec wrapping Reth's `ChainSpec` with our hardforks.
#[derive(Debug, Clone)]
pub struct CustomChainSpec {
    pub inner: ChainSpec,
}

impl CustomChainSpec {
    /// Creates a new custom chain spec with default hardfork config.
    pub fn new() -> Self {
        let config = CustomHardforkConfig::default();
        Self::with_config(config)
    }

    /// Creates a custom chain spec with provided hardfork config.
    pub fn with_config(config: CustomHardforkConfig) -> Self {
        let inner = ChainSpec::builder()
            .chain(Chain::mainnet())
            .genesis(Genesis::default())
            .frontier_activated()
            .homestead_activated()
            .with_forks(config.into_hardforks())
            .build();
        Self { inner }
    }
}

// Implement `Hardforks` to integrate custom hardforks with Reth's system.
impl Hardforks for CustomChainSpec {
    fn fork<H: Hardfork>(&self, fork: H) -> ForkCondition {
        self.inner.fork(fork)
    }

    fn forks_iter(&self) -> impl Iterator<Item = (&dyn Hardfork, ForkCondition)> {
        self.inner.forks_iter()
    }

    fn fork_id(&self, head: &reth_chainspec::Head) -> reth_chainspec::ForkId {
        self.inner.fork_id(head)
    }

    fn latest_fork_id(&self) -> reth_chainspec::ForkId {
        self.inner.latest_fork_id()
    }

    fn fork_filter(&self, head: reth_chainspec::Head) -> reth_chainspec::ForkFilter {
        self.inner.fork_filter(head)
    }
}

// Implement `EthChainSpec` for compatibility with Ethereum-based nodes.
impl EthChainSpec for CustomChainSpec {
    type Header = alloy_consensus::Header;

    fn chain(&self) -> Chain {
        self.inner.chain()
    }

    fn base_fee_params_at_timestamp(&self, timestamp: u64) -> reth_ethereum::chainspec::BaseFeeParams {
        self.inner.base_fee_params_at_timestamp(timestamp)
    }

    fn blob_params_at_timestamp(&self, timestamp: u64) -> Option<alloy_eips::eip7840::BlobParams> {
        self.inner.blob_params_at_timestamp(timestamp)
    }

    fn deposit_contract(&self) -> Option<&reth_ethereum::chainspec::DepositContract> {
        self.inner.deposit_contract()
    }

    fn genesis_hash(&self) -> revm_primitives::B256 {
        self.inner.genesis_hash()
    }

    fn prune_delete_limit(&self) -> usize {
        self.inner.prune_delete_limit()
    }

    fn display_hardforks(&self) -> Box<dyn core::fmt::Display> {
        Box::new(self.inner.display_hardforks())
    }

    fn genesis_header(&self) -> &Self::Header {
        self.inner.genesis_header()
    }

    fn genesis(&self) -> &Genesis {
        self.inner.genesis()
    }

    fn bootnodes(&self) -> Option<Vec<NodeRecord>> {
        self.inner.bootnodes()
    }

    fn final_paris_total_difficulty(&self) -> Option<revm_primitives::U256> {
        self.inner.final_paris_total_difficulty()
    }
}

// Implement `EthereumHardforks` to support Ethereum hardfork queries.
impl EthereumHardforks for CustomChainSpec {
    fn ethereum_fork_activation(&self, fork: EthereumHardfork) -> ForkCondition {
        self.inner.ethereum_fork_activation(fork)
    }
}