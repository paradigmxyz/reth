//! Partial Statelessness ExEx — reth Execution Extension that maintains a
//! network-level state cache and reports witness requirements per block.
//!
//! Run with:
//!   cargo run -p partial-stateless-exex -- node --chain mainnet --datadir /path/to/data
//!
//! This ExEx subscribes to canonical chain commits and:
//! 1. Extracts `BlockAccessedState` from each block's `BundleState`
//! 2. Updates the `NetworkStateCache` with the accessed state
//! 3. Computes and logs cache miss ratio (= witness requirement)

use futures::TryStreamExt;
use partial_stateless::{
    accessed_state::BlockAccessedState, network_cache::NetworkStateCache, policy::LastNBlocksPolicy,
};
use reth_ethereum::{
    exex::{ExExContext, ExExEvent, ExExNotification},
    node::{
        api::{FullNodeComponents, NodeTypes},
        builder::NodeHandleFor,
        EthereumNode,
    },
    EthPrimitives,
};
use tracing::{info, warn};

/// Configuration for the partial statelessness cache.
struct CacheConfig {
    /// Window size for account eviction policy (in blocks).
    account_window: u64,
    /// Window size for storage/code eviction policy (in blocks).
    storage_window: u64,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self { account_window: 20, storage_window: 10 }
    }
}

/// The ExEx function that processes chain notifications and maintains the cache.
async fn partial_stateless_exex<
    Node: FullNodeComponents<Types: NodeTypes<Primitives = EthPrimitives>>,
>(
    mut ctx: ExExContext<Node>,
    config: CacheConfig,
) -> eyre::Result<()> {
    let mut cache = NetworkStateCache::new(
        Box::new(LastNBlocksPolicy::new(config.account_window)),
        Box::new(LastNBlocksPolicy::new(config.storage_window)),
    );

    info!(
        target: "partial_stateless",
        account_window = config.account_window,
        storage_window = config.storage_window,
        "Partial Stateless ExEx started — monitoring cache state per block"
    );

    while let Some(notification) = ctx.notifications.try_next().await? {
        match &notification {
            ExExNotification::ChainCommitted { new } => {
                let execution_outcome = new.execution_outcome();
                let bundle = &execution_outcome.bundle;

                // Extract accessed state from BundleState
                let accessed = BlockAccessedState::from_bundle(bundle);

                // Get the block range in this commit
                let range = new.range();
                let tip_block = *range.end();

                // Compute miss BEFORE updating cache (simulates what a validator would see)
                let miss = cache.compute_miss(&accessed);

                // Now update the cache
                let stats = cache.on_block_executed(tip_block, &accessed);
                let snapshot = cache.snapshot();

                // Log comprehensive info
                info!(
                    target: "partial_stateless",
                    block = tip_block,
                    chain_range = ?range,
                    "═══════════════════════════════════════════════════"
                );
                info!(
                    target: "partial_stateless",
                    block = tip_block,
                    accessed_accounts = accessed.accounts.len(),
                    accessed_storage = accessed.storage.len(),
                    accessed_codes = accessed.codes.len(),
                    total_accessed = accessed.total_keys(),
                    "Block state access"
                );
                info!(
                    target: "partial_stateless",
                    block = tip_block,
                    miss_ratio = format!("{:.1}%", miss.miss_ratio * 100.0),
                    missed_accounts = miss.missed_accounts.len(),
                    missed_storage = miss.missed_storage.len(),
                    missed_codes = miss.missed_codes.len(),
                    total_missed = miss.total_missed,
                    "Witness requirement (cache miss)"
                );
                info!(
                    target: "partial_stateless",
                    block = tip_block,
                    cache_accounts = snapshot.total_accounts,
                    cache_storage = snapshot.total_storage_slots,
                    cache_codes = snapshot.total_codes,
                    estimated_memory = format_bytes(cache.estimated_memory_bytes()),
                    accounts_added = stats.accounts_added,
                    accounts_refreshed = stats.accounts_refreshed,
                    accounts_evicted = stats.accounts_evicted,
                    storage_added = stats.storage_added,
                    storage_refreshed = stats.storage_refreshed,
                    storage_evicted = stats.storage_evicted,
                    "Cache state after update"
                );

                // Log some sample missed accounts for inspection
                if !miss.missed_accounts.is_empty() {
                    let sample: Vec<_> = miss.missed_accounts.iter().take(5).collect();
                    info!(
                        target: "partial_stateless",
                        block = tip_block,
                        sample_missed_accounts = ?sample,
                        "Sample missed accounts (first 5)"
                    );
                }
            }
            ExExNotification::ChainReorged { old, new } => {
                warn!(
                    target: "partial_stateless",
                    from_chain = ?old.range(),
                    to_chain = ?new.range(),
                    "Chain reorg detected — cache may be stale, rebuilding from new chain"
                );

                // On reorg, re-process the new chain
                let execution_outcome = new.execution_outcome();
                let bundle = &execution_outcome.bundle;
                let accessed = BlockAccessedState::from_bundle(bundle);
                let tip_block = *new.range().end();

                cache.on_block_executed(tip_block, &accessed);
            }
            ExExNotification::ChainReverted { old } => {
                warn!(
                    target: "partial_stateless",
                    reverted_chain = ?old.range(),
                    "Chain reverted — note: cache is not rolled back in this PoC"
                );
            }
        }

        // Acknowledge processed height
        if let Some(committed_chain) = notification.committed_chain() {
            ctx.events.send(ExExEvent::FinishedHeight(committed_chain.tip().num_hash()))?;
        }
    }

    Ok(())
}

/// Format bytes into human-readable string.
fn format_bytes(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn main() -> eyre::Result<()> {
    reth_ethereum::cli::Cli::parse_args().run(async move |builder, _| {
        let config = CacheConfig::default();

        let handle: NodeHandleFor<EthereumNode> = builder
            .node(EthereumNode::default())
            .install_exex("partial-stateless", move |ctx| async move {
                Ok(partial_stateless_exex(ctx, config))
            })
            .launch()
            .await?;

        handle.wait_for_node_exit().await
    })
}
