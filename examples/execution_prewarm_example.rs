//! Example demonstrating how to use ExecutionStage with prewarming.
//!
//! This example shows how to create an ExecutionStage with prewarming enabled,
//! which spawns background threads to speculatively execute future blocks
//! and warm the cache for faster main execution.
//!
//! # Running on a reth box
//!
//! To test prewarming on a reth box during staged sync, you need to modify
//! the pipeline builder to use `ExecutionStage::new_with_prewarm()`.
//!
//! ## Quick Test (modify setup.rs)
//!
//! In `crates/node/builder/src/setup.rs`, replace the ExecutionStage creation:
//!
//! ```rust,ignore
//! // Original:
//! .set(ExecutionStage::new(
//!     evm_config,
//!     consensus,
//!     stage_config.execution.into(),
//!     stage_config.execution_external_clean_threshold(),
//!     exex_manager_handle,
//! ))
//!
//! // With prewarming (8GB cache):
//! .set(ExecutionStage::new_with_prewarm(
//!     evm_config,
//!     consensus,
//!     stage_config.execution.into(),
//!     stage_config.execution_external_clean_threshold(),
//!     exex_manager_handle,
//!     provider_factory.clone(),
//!     8_000_000_000, // 8GB cache
//! ))
//! ```
//!
//! ## Cache Size Recommendations
//!
//! - Small test: 1GB = 1_000_000_000
//! - Medium: 4GB = 4_000_000_000
//! - Large (production): 8GB = 8_000_000_000
//! - Very large: 16GB = 16_000_000_000
//!
//! ## Metrics to Monitor
//!
//! The prewarming module exposes these metrics:
//!
//! - `sync.stages.execution.prewarm.prewarm_blocks_started` - Blocks prewarmed
//! - `sync.stages.execution.prewarm.prewarm_blocks_completed` - Completed
//! - `sync.stages.execution.prewarm.prewarm_transactions_executed` - Txs executed
//! - `sync.stages.execution.prewarm.prewarm_duration` - Prewarm time per block
//!
//! Cache metrics:
//! - `sync.stages.execution.caching.account_cache_hits/misses`
//! - `sync.stages.execution.caching.storage_cache_hits/misses`
//! - `sync.stages.execution.caching.code_cache_hits/misses`

use reth_consensus::FullConsensus;
use reth_evm::ConfigureEvm;
use reth_exex::ExExManagerHandle;
use reth_provider::StateProviderFactory;
use reth_stages::stages::ExecutionStage;
use reth_stages_api::ExecutionStageThresholds;
use std::sync::Arc;

/// Example of creating an ExecutionStage with prewarming.
///
/// This function demonstrates the API but is not meant to be run directly.
/// Use it as a reference for integrating prewarming into your node.
#[allow(dead_code)]
fn create_prewarm_stage<E, F, N>(
    evm_config: E,
    consensus: Arc<dyn FullConsensus<N>>,
    thresholds: ExecutionStageThresholds,
    external_clean_threshold: u64,
    exex_manager_handle: ExExManagerHandle<N>,
    provider_factory: F,
    cache_size_bytes: u64,
) -> impl reth_stages_api::Stage<F>
where
    E: ConfigureEvm<Primitives = N> + Clone + Send + Sync + 'static,
    F: StateProviderFactory + Clone + Send + Sync + 'static,
    N: reth_primitives_traits::NodePrimitives,
{
    ExecutionStage::new_with_prewarm(
        evm_config,
        consensus,
        thresholds,
        external_clean_threshold,
        exex_manager_handle,
        provider_factory,
        cache_size_bytes,
    )
}

fn main() {
    println!("ExecutionStage Prewarming Example");
    println!("=================================");
    println!();
    println!("This example demonstrates the prewarming API.");
    println!("See the module documentation for usage instructions.");
    println!();
    println!("To test on a reth box:");
    println!("1. Modify crates/node/builder/src/setup.rs");
    println!("2. Replace ExecutionStage::new() with ExecutionStage::new_with_prewarm()");
    println!("3. Run: cargo build --release -p reth");
    println!("4. Monitor metrics: sync.stages.execution.prewarm.*");
}
