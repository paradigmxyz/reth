//! Main entry point for the custom hardforks example.
//!
//! This sets up a custom chain spec with hardforks and demonstrates feature gating.

use crate::chainspec::CustomChainSpec;
use reth_chainspec::Hardforks;
use std::sync::Arc;

mod chainspec;
mod hardforks;

fn main() {
    // Create a custom chain spec with default hardfork config.
    let chain_spec = Arc::new(CustomChainSpec::new());

    // Log hardfork activation for demonstration.
    println!("Custom hardforks configured:");
    for (fork, condition) in chain_spec.forks_iter() {
        println!("  - {}: {:?}", fork.name(), condition);
    }

    println!("Example: Custom hardforks are integrated into the chain spec.");
    println!("To use in EVM, check activation with chain_spec.is_fork_active_at_block(CustomHardfork::BasicUpgrade, block_number)");
}