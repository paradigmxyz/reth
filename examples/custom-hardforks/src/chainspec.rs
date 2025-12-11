//! Custom chain specification integrating hardforks.
//!
//! This demonstrates how to build a `ChainSpec` with custom hardforks,
//! implementing required traits for integration with Reth's chain management.

use alloy_eips::eip7840::BlobParams;
use alloy_genesis::Genesis;
use alloy_primitives::{B256, U256};
use reth_chainspec::{
    hardfork, BaseFeeParams, Chain, ChainSpec, DepositContract, EthChainSpec, EthereumHardfork,
    EthereumHardforks, ForkCondition, Hardfork, Hardforks,
};
use reth_network_peers::NodeRecord;
use serde::{Deserialize, Serialize};

// Define custom hardfork variants using Reth's `hardfork!` macro.
// Each variant represents a protocol upgrade (e.g., enabling new features).
hardfork!(
    /// Custom hardforks for the example chain.
    ///
    /// These are inspired by Ethereum's upgrades but customized for demonstration.
    /// Add new variants here to extend the chain's hardfork set.
    CustomHardfork {
        /// Enables basic custom features (e.g., a new precompile).
        BasicUpgrade,
        /// Enables advanced features (e.g., state modifications).
        AdvancedUpgrade,
    }
);

// Implement the `Hardfork` trait for each variant.
// This defines the name and any custom logic (e.g., feature toggles).
// Note: The hardfork! macro already implements Hardfork, so no manual impl needed.

// Configuration for hardfork activation.
// This struct holds settings like activation blocks and is serializable for config files.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CustomHardforkConfig {
    /// Block number to activate BasicUpgrade.
    pub basic_upgrade_block: u64,
    /// Block number to activate AdvancedUpgrade.
    pub advanced_upgrade_block: u64,
}

// Custom chain spec wrapping Reth's `ChainSpec` with our hardforks.
#[derive(Debug, Clone)]
pub struct CustomChainSpec {
    pub inner: ChainSpec,
}

impl CustomChainSpec {
    /// Creates a custom chain spec from a genesis file.
    ///
    /// This parses the [`ChainSpec`] and adds the custom hardforks.
    pub fn from_genesis(genesis: Genesis) -> Self {
        let extra = genesis.config.extra_fields.deserialize_as::<CustomHardforkConfig>().unwrap();

        let mut inner = ChainSpec::from_genesis(genesis);
        inner.hardforks.insert(
            CustomHardfork::BasicUpgrade,
            ForkCondition::Timestamp(extra.basic_upgrade_block),
        );
        inner.hardforks.insert(
            CustomHardfork::AdvancedUpgrade,
            ForkCondition::Timestamp(extra.advanced_upgrade_block),
        );
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

    fn base_fee_params_at_timestamp(&self, timestamp: u64) -> BaseFeeParams {
        self.inner.base_fee_params_at_timestamp(timestamp)
    }

    fn blob_params_at_timestamp(&self, timestamp: u64) -> Option<BlobParams> {
        self.inner.blob_params_at_timestamp(timestamp)
    }

    fn deposit_contract(&self) -> Option<&DepositContract> {
        self.inner.deposit_contract()
    }

    fn genesis_hash(&self) -> B256 {
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

    fn final_paris_total_difficulty(&self) -> Option<U256> {
        self.inner.final_paris_total_difficulty()
    }
}

// Implement `EthereumHardforks` to support Ethereum hardfork queries.
impl EthereumHardforks for CustomChainSpec {
    fn ethereum_fork_activation(&self, fork: EthereumHardfork) -> ForkCondition {
        self.inner.ethereum_fork_activation(fork)
    }
}
