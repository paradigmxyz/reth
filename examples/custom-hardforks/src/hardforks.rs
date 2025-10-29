//! Defines custom hardforks for the example chain.
//!
//! This module demonstrates defining hardfork variants, implementing the `Hardfork` trait,
//! and providing a configuration structure for activation settings.

use reth_chainspec::hardfork;
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomHardforkConfig {
    /// Block number to activate BasicUpgrade.
    pub basic_upgrade_block: Option<u64>,
    /// Block number to activate AdvancedUpgrade.
    pub advanced_upgrade_block: Option<u64>,
}

impl Default for CustomHardforkConfig {
    fn default() -> Self {
        Self {
            basic_upgrade_block: Some(10), // Activate at block 10 for demo
            advanced_upgrade_block: Some(20),
        }
    }
}

impl CustomHardforkConfig {
    /// Converts the config into a `ChainHardforks` for integration.
    /// This builds the hardfork list with activation conditions.
    pub fn into_hardforks(self) -> reth_chainspec::ChainHardforks {
        let mut hardforks = reth_chainspec::ChainHardforks::default();
        if let Some(block) = self.basic_upgrade_block {
            hardforks
                .insert(CustomHardfork::BasicUpgrade, reth_chainspec::ForkCondition::Block(block));
        }
        if let Some(block) = self.advanced_upgrade_block {
            hardforks.insert(
                CustomHardfork::AdvancedUpgrade,
                reth_chainspec::ForkCondition::Block(block),
            );
        }
        hardforks
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_chainspec::ForkCondition;

    #[test]
    fn hardfork_names() {
        assert_eq!(CustomHardfork::BasicUpgrade.name(), "BasicUpgrade");
        assert_eq!(CustomHardfork::AdvancedUpgrade.name(), "AdvancedUpgrade");
    }

    #[test]
    fn config_to_hardforks() {
        let config =
            CustomHardforkConfig { basic_upgrade_block: Some(5), advanced_upgrade_block: Some(15) };
        let hardforks = config.into_hardforks();
        assert_eq!(hardforks.fork(CustomHardfork::BasicUpgrade), ForkCondition::Block(5));
        assert_eq!(hardforks.fork(CustomHardfork::AdvancedUpgrade), ForkCondition::Block(15));
    }
}
