//! Main entry point for the custom hardforks example.
//!
//! This sets up a custom chain spec with hardforks loaded from genesis and demonstrates feature
//! gating.

use crate::chainspec::CustomChainSpec;
use alloy_genesis::Genesis;
use reth_chainspec::Hardforks;
use std::{fs, sync::Arc};

mod chainspec;
mod hardforks;

fn main() {
    // Load genesis from file
    let genesis_content = fs::read_to_string("genesis.json").expect("Failed to read genesis.json");
    let genesis: Genesis =
        serde_json::from_str(&genesis_content).expect("Failed to parse genesis.json");

    // Create a custom chain spec from genesis
    let chain_spec = Arc::new(CustomChainSpec::from_genesis(genesis));

    // Log hardfork activation for demonstration.
    println!("Custom hardforks configured:");
    for (fork, condition) in chain_spec.forks_iter() {
        println!("  - {}: {:?}", fork.name(), condition);
    }

    println!("Example: Custom hardforks are integrated into the chain spec.");
    println!("To use in EVM, check activation with chain_spec.is_fork_active_at_block(CustomHardfork::BasicUpgrade, block_number)");
}
