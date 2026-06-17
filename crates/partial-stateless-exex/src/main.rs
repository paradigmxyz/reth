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
//! 4. Computes actual Merkle proof (witness) size for cache-missed state

use futures::TryStreamExt;
use partial_stateless::{
    accessed_state::BlockAccessedState,
    network_cache::NetworkStateCache,
    persistence::{load_from_file, save_to_file},
    policy::LastNBlocksPolicy,
    witness::{measure_multiproof_size, build_sidecar_targets},
    PartialStatelessSidecar, SerializableMultiProof,
};
use reth_ethereum::{
    chainspec::EthChainSpec,
    exex::{ExExContext, ExExEvent, ExExNotification},
    node::{
        api::{FullNodeComponents, NodeTypes},
        builder::NodeHandleFor,
        EthereumNode,
    },
    provider::StateProviderFactory,
    storage::StateProofProvider,
    EthPrimitives,
};
use reth_provider::HeaderProvider;
use reth_trie_common::TrieInput;
use alloy_primitives::Bytes;
use std::time::Instant;
use std::fs;
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
        Self { account_window: 60, storage_window: 30 }
    }
}

/// The ExEx function that processes chain notifications and maintains the cache.
async fn partial_stateless_exex<
    Node: FullNodeComponents<Types: NodeTypes<Primitives = EthPrimitives>>,
>(
    mut ctx: ExExContext<Node>,
    config: CacheConfig,
) -> eyre::Result<()> {
    // Resolve the cache file path: datadir/partial_stateless_cache.bin
    let cache_dir = ctx.config.datadir.clone().resolve_datadir(ctx.config.chain.chain());
    let cache_path = cache_dir.as_ref().join("partial_stateless_cache.bin");

    // Cache coherence guarantee: the LastNBlocksPolicy is fully deterministic —
    // given the same canonical chain and window parameters, all peers converge
    // to the same cache state. Loading from disk and replaying missed blocks
    // during sync produces an identical result to continuous operation.
    let mut cache = if cache_path.exists() {
        match load_from_file(
            &cache_path,
            Box::new(LastNBlocksPolicy::new(config.account_window)),
            Box::new(LastNBlocksPolicy::new(config.storage_window)),
        ) {
            Ok(loaded_cache) => {
                let cache_block = loaded_cache.current_block();
                let head_block = ctx.head.number;

                // Validation: Gap Tolerance based on config.account_window
                let max_allowed_gap = config.account_window;
                if cache_block <= head_block && head_block - cache_block <= max_allowed_gap {
                    info!(
                        target: "partial_stateless",
                        cache_block = cache_block,
                        head_block = head_block,
                        gap = head_block - cache_block,
                        "Warm state cache loaded successfully from disk. Continuing sync..."
                    );
                    loaded_cache
                } else {
                    warn!(
                        target: "partial_stateless",
                        cache_block = cache_block,
                        head_block = head_block,
                        max_allowed_gap = max_allowed_gap,
                        "Cache file block state is too far from head block or in the future. Starting with cold cache."
                    );
                    NetworkStateCache::new(
                        Box::new(LastNBlocksPolicy::new(config.account_window)),
                        Box::new(LastNBlocksPolicy::new(config.storage_window)),
                    )
                }
            }
            Err(e) => {
                warn!(
                    target: "partial_stateless",
                    error = %e,
                    "Failed to load cache file from disk. Starting with cold cache."
                );
                NetworkStateCache::new(
                    Box::new(LastNBlocksPolicy::new(config.account_window)),
                    Box::new(LastNBlocksPolicy::new(config.storage_window)),
                )
            }
        }
    } else {
        info!(
            target: "partial_stateless",
            "No existing cache file found at {}. Starting with cold cache.",
            cache_path.display()
        );
        NetworkStateCache::new(
            Box::new(LastNBlocksPolicy::new(config.account_window)),
            Box::new(LastNBlocksPolicy::new(config.storage_window)),
        )
    };

    info!(
        target: "partial_stateless",
        account_window = config.account_window,
        storage_window = config.storage_window,
        cache_path = %cache_path.display(),
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

                // === Phase 2: Compute actual witness (Merkle proof) size & Generate Sidecar ===
                if miss.total_missed > 0 {
                    // Extract raw targets and hashed multiproof targets in one pass
                    let (raw_targets, targets) = build_sidecar_targets(&miss);
                    let target_accounts = targets.len();
                    let target_slots: usize = targets.values().map(|slots| slots.len()).sum();

                    // Calculate total bytes of missed bytecodes
                    let missed_bytecode_bytes: usize = miss.missed_codes
                        .iter()
                        .filter_map(|code_hash| accessed.codes.get(code_hash))
                        .map(|bytes| bytes.len())
                        .sum();

                    let missed_bytecodes: Vec<Bytes> = miss.missed_codes
                        .iter()
                        .filter_map(|code_hash| accessed.codes.get(code_hash).cloned())
                        .collect();

                    // Get state provider for the parent block (proof against pre-execution state)
                    // We use tip_block - 1 because the witness proves state BEFORE this block
                    let witness_result = if tip_block > 0 {
                        let start = Instant::now();
                        match ctx.provider().history_by_block_number(tip_block - 1) {
                            Ok(state_provider) => {
                                // Compute multiproof with empty TrieInput (proof against DB state)
                                match state_provider.multiproof(TrieInput::default(), targets) {
                                    Ok(proof) => {
                                        let elapsed_ms = start.elapsed().as_millis() as u64;
                                        let mut result = measure_multiproof_size(&proof, missed_bytecode_bytes);
                                        result.computation_time_ms = Some(elapsed_ms);
                                        result.target_accounts = target_accounts;
                                        result.target_storage_slots = target_slots;

                                        // --- Generate and Save Sidecar ---
                                        let sidecar_generation_result = 'sidecar: {
                                            // Get tip block header info
                                            let Some(block) = new.blocks().get(&tip_block) else {
                                                break 'sidecar Err(eyre::eyre!("Tip block {} not found in commit blocks", tip_block));
                                            };

                                            let parent_hash = block.parent_hash;
                                            let parent_header = match ctx.provider().sealed_header_by_hash(parent_hash) {
                                                Ok(Some(h)) => h,
                                                Ok(None) => break 'sidecar Err(eyre::eyre!("Parent header not found for hash {:?}", parent_hash)),
                                                Err(e) => break 'sidecar Err(eyre::eyre!("Failed to fetch parent header: {:?}", e)),
                                            };
                                            let parent_state_root = parent_header.state_root;

                                            // Check cache coherency
                                            // Since cache.on_block_executed(tip_block) has already been called,
                                            // the cache block should be at `tip_block`.
                                            if cache.current_block() != tip_block {
                                                warn!(
                                                    target: "partial_stateless",
                                                    block = tip_block,
                                                    cache_block = cache.current_block(),
                                                    expected_block = tip_block,
                                                    "Cache state mismatch: cache block is not synced to tip block number. Skipping sidecar generation."
                                                );
                                                break 'sidecar Ok(());
                                            }

                                            // Convert proof to serializable format
                                            let serializable_proof = SerializableMultiProof::from_multiproof(&proof);
                                            let serialized_multiproof = match bincode::serialize(&serializable_proof) {
                                                Ok(b) => b,
                                                Err(e) => break 'sidecar Err(eyre::eyre!("Failed to serialize multiproof: {:?}", e)),
                                            };

                                            let sidecar = PartialStatelessSidecar {
                                                parent_hash,
                                                parent_state_root,
                                                block_hash: block.hash(),
                                                block_number: tip_block,
                                                cache_block: tip_block - 1,
                                                cache_policy_metadata: format!(
                                                    "LastNBlocks(account: {}, storage/code: {})",
                                                    config.account_window, config.storage_window
                                                ),
                                                raw_targets: raw_targets.clone(),
                                                serialized_multiproof,
                                                missed_bytecodes: missed_bytecodes.clone(),
                                                stats: result.clone(),
                                            };

                                            // Ensure sidecar directory exists under workspace root
                                            // Resolves to ./sidecar from the process current working directory
                                            let sidecar_dir = std::env::current_dir()
                                                .unwrap_or_else(|_| std::path::PathBuf::from("."))
                                                .join("sidecar");
                                            if let Err(e) = fs::create_dir_all(&sidecar_dir) {
                                                break 'sidecar Err(eyre::eyre!("Failed to create sidecar directory: {:?}", e));
                                            }

                                            let sidecar_filename = format!("block_{}_{:?}.bin", tip_block, block.hash());
                                            let sidecar_path = sidecar_dir.join(sidecar_filename);

                                            let sidecar_bytes = match bincode::serialize(&sidecar) {
                                                Ok(b) => b,
                                                Err(e) => break 'sidecar Err(eyre::eyre!("Failed to serialize sidecar: {:?}", e)),
                                            };

                                            if let Err(e) = fs::write(&sidecar_path, sidecar_bytes) {
                                                break 'sidecar Err(eyre::eyre!("Failed to write sidecar file {:?}: {:?}", sidecar_path, e));
                                            }

                                            info!(
                                                target: "partial_stateless",
                                                block = tip_block,
                                                path = %sidecar_path.display(),
                                                size = format_bytes(fs::metadata(&sidecar_path).map(|m| m.len() as usize).unwrap_or(0)),
                                                "Saved witness sidecar successfully"
                                            );

                                            Ok(())
                                        };

                                        if let Err(e) = sidecar_generation_result {
                                            warn!(
                                                target: "partial_stateless",
                                                block = tip_block,
                                                error = %e,
                                                "Sidecar generation failed (non-fatal)"
                                            );
                                        }

                                        Some(result)
                                    }
                                    Err(e) => {
                                        warn!(
                                            target: "partial_stateless",
                                            block = tip_block,
                                            error = %e,
                                            "Failed to compute multiproof"
                                        );
                                        None
                                    }
                                }
                            }
                            Err(e) => {
                                warn!(
                                    target: "partial_stateless",
                                    block = tip_block,
                                    error = %e,
                                    "Failed to get state provider for witness computation"
                                );
                                None
                            }
                        }
                    } else {
                        None
                    };

                    if let Some(witness) = witness_result {
                        info!(
                            target: "partial_stateless",
                            block = tip_block,
                            witness_total_bytes = witness.total_size_bytes,
                            witness_total = format_bytes(witness.total_size_bytes),
                            account_proof_bytes = witness.account_proof_bytes,
                            account_proof = format_bytes(witness.account_proof_bytes),
                            storage_proof_bytes = witness.storage_proof_bytes,
                            storage_proof = format_bytes(witness.storage_proof_bytes),
                            bytecode_bytes = witness.bytecode_bytes,
                            bytecode_size = format_bytes(witness.bytecode_bytes),
                            account_proof_nodes = witness.account_proof_nodes,
                            storage_proof_nodes = witness.storage_proof_nodes,
                            target_accounts = witness.target_accounts,
                            target_storage_slots = witness.target_storage_slots,
                            computation_time_ms = witness.computation_time_ms.unwrap_or(0),
                            "Witness size (Merkle proof)"
                        );
                    }
                } else {
                    info!(
                        target: "partial_stateless",
                        block = tip_block,
                        "No witness needed (100% cache hit)"
                    );
                }

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

                // Save updated cache state to file
                if let Err(e) = save_to_file(&cache, &cache_path) {
                    warn!(
                        target: "partial_stateless",
                        block = tip_block,
                        error = %e,
                        "Failed to save cache state to disk"
                    );
                }

                // // Log some sample missed accounts for inspection
                // if !miss.missed_accounts.is_empty() {
                //     let sample: Vec<_> = miss.missed_accounts.iter().take(5).collect();
                //     info!(
                //         target: "partial_stateless",
                //         block = tip_block,
                //         sample_missed_accounts = ?sample,
                //         "Sample missed accounts (first 5)"
                //     );
                // }
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
