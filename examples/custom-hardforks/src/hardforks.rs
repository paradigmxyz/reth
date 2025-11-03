//! Defines custom hardforks for the example chain.
//!
//! This module demonstrates defining hardfork variants, implementing the `Hardfork` trait,
//! and providing a configuration structure for activation settings.

use alloy_eip2124::ForkFilterKey;
use reth_chainspec::{hardfork, ForkCondition, Hardfork, Hardforks};
use std::any::Any;
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
    pub basic_upgrade_block: Option<u64>,
    /// Block number to activate AdvancedUpgrade.
    pub advanced_upgrade_block: Option<u64>,
}

impl Hardforks for CustomHardforkConfig {
    fn fork<H: Hardfork>(&self, fork: H) -> ForkCondition {
        if let Some(hardfork) = (&fork as &dyn Any).downcast_ref::<CustomHardfork>() {
            match hardfork {
                CustomHardfork::BasicUpgrade => {
                    self.basic_upgrade_block.map(ForkCondition::Block).unwrap_or(ForkCondition::Never)
                }
                CustomHardfork::AdvancedUpgrade => {
                    self.advanced_upgrade_block.map(ForkCondition::Block).unwrap_or(ForkCondition::Never)
                }
            }
        } else {
            ForkCondition::Never
        }
    }

    fn forks_iter(&self) -> impl Iterator<Item = (&dyn Hardfork, ForkCondition)> {
        [
            (&CustomHardfork::BasicUpgrade as &dyn Hardfork, self.fork(CustomHardfork::BasicUpgrade)),
            (&CustomHardfork::AdvancedUpgrade as &dyn Hardfork, self.fork(CustomHardfork::AdvancedUpgrade)),
        ].into_iter()
    }

    fn fork_id(&self, head: &reth_chainspec::Head) -> reth_chainspec::ForkId {
        // Simplified, in practice you'd compute based on active forks
        reth_chainspec::ForkId { hash: head.hash.into(), next: 0 }
    }

    fn latest_fork_id(&self) -> reth_chainspec::ForkId {
        reth_chainspec::ForkId { hash: revm_primitives::B256::ZERO.into(), next: 0 }
    }

    fn fork_filter(&self, head: reth_chainspec::Head) -> reth_chainspec::ForkFilter {
        reth_chainspec::ForkFilter::new(head, head.hash, head.timestamp, self.forks_iter().filter_map(|(_, c)| c.block_number().map(ForkFilterKey::Block)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hardfork_names() {
        assert_eq!(CustomHardfork::BasicUpgrade.name(), "BasicUpgrade");
        assert_eq!(CustomHardfork::AdvancedUpgrade.name(), "AdvancedUpgrade");
    }
}
