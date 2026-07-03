//! Partial Statelessness ExEx — reth Execution Extension that maintains a
//! network-level state cache and reports witness requirements per block.
//!
//! Run with:
//!   cargo run -p partial-stateless-exex -- node --chain mainnet --datadir /path/to/data
//!
//! This ExEx subscribes to canonical chain commits and:
//! 1. Extracts `BlockAccessedState` via EVM simulation of each block
//! 2. Updates the `NetworkStateCache` with the accessed state
//! 3. Computes and logs cache miss ratio (= witness requirement)
//! 4. Computes actual Merkle proof (witness) size for cache-missed state

use futures::TryStreamExt;
use partial_stateless::{
    accessed_state::BlockAccessedState,
    fixture::{save_fixture, AccessedStateFixture},
    network_cache::NetworkStateCache,
    persistence::{load_from_file, save_to_file},
    policy::LastNBlocksPolicy,
    witness::{
        accessed_to_state_targets, build_sidecar_targets, cache_hit_targets,
        measure_multiproof_size, state_targets_to_proof_targets, WitnessResult,
    },
    CacheFootprintStats, PartialExecutionWitness, PartialExecutionWitnessState,
    PartialStatelessSidecar, PartitionCheck, SerializableMultiProof, SidecarBenchmarkManifest,
    StateTargetSet, WitnessReductionStats,
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
use reth_provider::{BlockIdReader, HeaderProvider};
use reth_trie_common::TrieInput;
use reth_evm::{ConfigureEvm, execute::Executor};
use reth_revm::database::StateProviderDatabase;
use revm::database::State;
use alloy_primitives::Bytes;
use alloy_rlp::Encodable;
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

    // Optional reproducible-dataset capture: when `PS_CAPTURE_DIR` is set, dump the
    // per-block `BlockAccessedState` (the only cache input) so the cache-window
    // benchmark can replay a fixed range offline, with no node or EVM. This reuses
    // the exact execution path the live system uses, so the dataset is faithful.
    let capture_dir = std::env::var("PS_CAPTURE_DIR").ok().map(std::path::PathBuf::from);
    if let Some(dir) = &capture_dir {
        info!(
            target: "partial_stateless",
            dir = %dir.display(),
            "Accessed-state fixture capture ENABLED (PS_CAPTURE_DIR) — run until ~300 blocks captured"
        );
    }

    // Optional benchmark-only comparison against the FULL witness (every accessed
    // key, ignoring the cache). It computes a second, larger multiproof per block,
    // so it is off by default and gated behind `PS_WITNESS_BASELINE`. Core sidecar
    // generation never depends on it.
    let compute_baseline = std::env::var("PS_WITNESS_BASELINE")
        .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes" | "on"))
        .unwrap_or(false);
    if compute_baseline {
        info!(
            target: "partial_stateless",
            "Full-witness baseline comparison ENABLED (PS_WITNESS_BASELINE) — extra multiproof per block"
        );
    }

    // Optional per-thread resource metrics (CPU time + page faults) captured
    // around the cold multiproof, to attribute its cost between compute and
    // disk I/O. Off by default and gated behind `PS_RESOURCE_METRICS`; when
    // disabled the getrusage syscalls are skipped and the metric fields stay None.
    let resource_metrics = std::env::var("PS_RESOURCE_METRICS")
        .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes" | "on"))
        .unwrap_or(false);
    if resource_metrics {
        info!(
            target: "partial_stateless",
            "Per-thread resource metrics ENABLED (PS_RESOURCE_METRICS) — cpu_time_ms + major/minor_page_faults per block"
        );
        #[cfg(not(target_os = "linux"))]
        warn!(
            target: "partial_stateless",
            "Per-thread CPU/page-fault metrics require Linux RUSAGE_THREAD; this platform will log default zeros for cpu_time_ms, major_page_faults, and minor_page_faults"
        );
        if compute_baseline {
            warn!(
                target: "partial_stateless",
                "PS_WITNESS_BASELINE runs before the partial multiproof; resource/page-fault metrics for the partial proof may be lower because the full baseline can warm the OS page cache"
            );
        }
    }

    while let Some(notification) = ctx.notifications.try_next().await? {
        match &notification {
            ExExNotification::ChainCommitted { new } => {
                let range = new.range();
                let tip_block = *range.end();

                // Process blocks in chronological order
                for (block_number, block) in new.blocks() {
                    let parent_block_number = block_number.saturating_sub(1);

                    // Get state provider for the parent block to run execution simulation
                    let state_provider = match ctx.provider().history_by_block_number(parent_block_number) {
                        Ok(provider) => provider,
                        Err(e) => {
                            warn!(
                                target: "partial_stateless",
                                block = *block_number,
                                error = %e,
                                "Failed to get state provider for parent block. Skipping block."
                            );
                            continue;
                        }
                    };

                    let state_provider_db = StateProviderDatabase::new(&state_provider);
                    let mut db = State::builder()
                        .with_bundle_update()
                        .with_database(state_provider_db)
                        .build();

                    let block_executor = ctx.evm_config().executor(&mut db);

                    let mut accessed = BlockAccessedState::default();
                    let mut lowest_block_number = None;

                    let sim_result = block_executor.execute_with_state_closure(block, |statedb: &State<_>| {
                        accessed = BlockAccessedState::from_simulated_state(statedb);
                        lowest_block_number = statedb.block_hashes.lowest().map(|(num, _)| num);
                    });

                    if let Err(e) = sim_result {
                        warn!(
                            target: "partial_stateless",
                            block = *block_number,
                            error = %e,
                            "Simulation failed for block. Skipping block."
                        );
                        continue;
                    }

                    // Capture the reproducible benchmark fixture (independent of the
                    // cache/sidecar pipeline below, so it survives later failures).
                    if let Some(dir) = &capture_dir {
                        let parent_state_root = ctx
                            .provider()
                            .sealed_header_by_hash(block.parent_hash)
                            .ok()
                            .flatten()
                            .map(|h| h.state_root)
                            .unwrap_or_default();
                        let fixture = AccessedStateFixture {
                            block_number: *block_number,
                            block_hash: block.hash(),
                            parent_state_root,
                            accessed: accessed.clone(),
                        };
                        match save_fixture(dir, &fixture) {
                            Ok(path) => info!(
                                target: "partial_stateless",
                                block = *block_number,
                                path = %path.display(),
                                accounts = accessed.accounts.len(),
                                storage = accessed.storage.len(),
                                codes = accessed.codes.len(),
                                "Captured accessed-state fixture"
                            ),
                            Err(e) => warn!(
                                target: "partial_stateless",
                                block = *block_number,
                                error = %e,
                                "Failed to capture accessed-state fixture"
                            ),
                        }
                    }

                    // Compute miss BEFORE updating cache (simulates what a validator would see)
                    let cache_snapshot_before = cache.snapshot();
                    let cache_memory_before = cache.estimated_memory_bytes();
                    let miss = cache.compute_miss(&accessed);
                    let accessed_targets = accessed_to_state_targets(&accessed);
                    let cache_hit_targets = cache_hit_targets(&accessed, &miss);

                    // Now update the cache
                    let stats = cache.on_block_executed(*block_number, &accessed);
                    let snapshot = cache.snapshot();
                    let cache_memory_after = cache.estimated_memory_bytes();

                    // Log comprehensive info
                    info!(
                        target: "partial_stateless",
                        block = *block_number,
                        "═══════════════════════════════════════════════════"
                    );
                    info!(
                        target: "partial_stateless",
                        block = *block_number,
                        accessed_accounts = accessed.accounts.len(),
                        accessed_storage = accessed.storage.len(),
                        accessed_codes = accessed.codes.len(),
                        total_accessed = accessed.total_keys(),
                        "Block state access"
                    );
                    info!(
                        target: "partial_stateless",
                        block = *block_number,
                        miss_ratio = format!("{:.1}%", miss.miss_ratio * 100.0),
                        missed_accounts = miss.missed_accounts.len(),
                        missed_storage = miss.missed_storage.len(),
                        missed_codes = miss.missed_codes.len(),
                        total_missed = miss.total_missed,
                        "Witness requirement (cache miss)"
                    );

                    // === Phase 2: Compute actual witness (Merkle proof) size & Generate Sidecar ===
                    let witness_result = {
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

                        // Optional full-witness baseline (every accessed key, ignoring the
                        // cache). Gated behind PS_WITNESS_BASELINE since it is an extra,
                        // larger multiproof. A failure here is non-fatal — it only drops the
                        // comparison for this block and never skips the real partial sidecar.
                        let full_sidecar_baseline_stats: Option<WitnessResult> = if compute_baseline {
                            let full_targets = state_targets_to_proof_targets(&accessed_targets);
                            let full_target_accounts = full_targets.len();
                            let full_target_slots: usize =
                                full_targets.values().map(|slots| slots.len()).sum();
                            let full_bytecode_bytes: usize =
                                accessed.codes.values().map(|bytes| bytes.len()).sum();
                            let full_start = Instant::now();
                            match state_provider.multiproof(TrieInput::default(), full_targets) {
                                Ok(full_proof) => {
                                    let elapsed_ms = full_start.elapsed().as_millis() as u64;
                                    let mut full_result =
                                        measure_multiproof_size(&full_proof, full_bytecode_bytes);
                                    full_result.computation_time_ms = Some(elapsed_ms);
                                    full_result.target_accounts = full_target_accounts;
                                    full_result.target_storage_slots = full_target_slots;
                                    Some(full_result)
                                }
                                Err(e) => {
                                    warn!(
                                        target: "partial_stateless",
                                        block = *block_number,
                                        error = %e,
                                        "Failed to compute full sidecar baseline multiproof (comparison dropped for this block)"
                                    );
                                    None
                                }
                            }
                        } else {
                            None
                        };

                        // Snapshot per-thread CPU + page faults so we can attribute the
                        // cold multiproof cost (compute-bound vs I/O/swap-bound) per block.
                        // Gated behind PS_RESOURCE_METRICS; when disabled the getrusage
                        // syscalls are skipped and the metric fields stay None.
                        let rusage_before = resource_metrics.then(thread_rusage);
                        let start = Instant::now();
                        // Compute multiproof with empty TrieInput (proof against DB state)
                        match state_provider.multiproof(TrieInput::default(), targets) {
                            Ok(proof) => {
                                let elapsed_ms = start.elapsed().as_millis() as u64;

                                let mut result = measure_multiproof_size(&proof, missed_bytecode_bytes);
                                result.computation_time_ms = Some(elapsed_ms);
                                if let Some((cpu_us_before, majflt_before, minflt_before)) = rusage_before {
                                    let (cpu_us_after, majflt_after, minflt_after) = thread_rusage();
                                    result.cpu_time_ms = Some(cpu_us_after.saturating_sub(cpu_us_before) / 1000);
                                    result.major_page_faults = Some(majflt_after.saturating_sub(majflt_before));
                                    result.minor_page_faults = Some(minflt_after.saturating_sub(minflt_before));
                                }
                                result.target_accounts = target_accounts;
                                result.target_storage_slots = target_slots;

                                // --- Generate and Save Sidecar ---
                                let sidecar_generation_result = 'sidecar: {
                                    let parent_hash = block.parent_hash;
                                    let parent_header = match ctx.provider().sealed_header_by_hash(parent_hash) {
                                        Ok(Some(h)) => h,
                                        Ok(None) => break 'sidecar Err(eyre::eyre!("Parent header not found for hash {:?}", parent_hash)),
                                        Err(e) => break 'sidecar Err(eyre::eyre!("Failed to fetch parent header: {:?}", e)),
                                    };
                                    let parent_state_root = parent_header.state_root;

                                    // Check cache coherency
                                    if cache.current_block() != *block_number {
                                        warn!(
                                            target: "partial_stateless",
                                            block = *block_number,
                                            cache_block = cache.current_block(),
                                            expected_block = *block_number,
                                            "Cache state mismatch: cache block is not synced to block number. Skipping sidecar generation."
                                        );
                                        break 'sidecar Ok(());
                                    }

                                    // Fetch ancestor headers based on BLOCKHASH usage
                                    let smallest = lowest_block_number.unwrap_or(block_number.saturating_sub(1));
                                    let ancestor_range = smallest..*block_number;
                                    let ancestor_headers: Vec<Bytes> = match ctx.provider().headers_range(ancestor_range) {
                                        Ok(headers) => headers
                                            .into_iter()
                                            .map(|header| {
                                                let mut buf = Vec::new();
                                                let _ = header.encode(&mut buf);
                                                buf.into()
                                            })
                                            .collect(),
                                        Err(e) => {
                                            break 'sidecar Err(eyre::eyre!("Failed to fetch ancestor headers for range: {:?}", e));
                                        }
                                    };

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
                                        block_number: *block_number,
                                        cache_block: block_number - 1,
                                        cache_policy_metadata: format!(
                                            "LastNBlocks(account: {}, storage/code: {})",
                                            config.account_window, config.storage_window
                                        ),
                                        miss_manifest: raw_targets.clone(),
                                        witness: PartialExecutionWitness {
                                            state: PartialExecutionWitnessState::MptMultiProof(
                                                serialized_multiproof,
                                            ),
                                            codes: missed_bytecodes.clone(),
                                            keys: raw_targets.key_preimages(),
                                            headers: ancestor_headers,
                                        },
                                        stats: result.clone(),
                                    };

                                    // Ensure sidecar directory exists under workspace root
                                    let sidecar_dir = std::env::current_dir()
                                        .unwrap_or_else(|_| std::path::PathBuf::from("."))
                                        .join("sidecar");
                                    if let Err(e) = fs::create_dir_all(&sidecar_dir) {
                                        break 'sidecar Err(eyre::eyre!("Failed to create sidecar directory: {:?}", e));
                                    }

                                    let sidecar_filename = format!("block_{}_{:?}.bin", block_number, block.hash());
                                    let sidecar_path = sidecar_dir.join(sidecar_filename);

                                    let sidecar_bytes = match bincode::serialize(&sidecar) {
                                        Ok(b) => b,
                                        Err(e) => break 'sidecar Err(eyre::eyre!("Failed to serialize sidecar: {:?}", e)),
                                    };

                                    if let Err(e) = fs::write(&sidecar_path, sidecar_bytes) {
                                        break 'sidecar Err(eyre::eyre!("Failed to write sidecar file {:?}: {:?}", sidecar_path, e));
                                    }

                                    let sidecar_bytes_len = fs::metadata(&sidecar_path)
                                        .map(|m| m.len() as usize)
                                        .unwrap_or(0);
                                    let sidecar_miss = StateTargetSet::from(&raw_targets);
                                    let partition =
                                        PartitionCheck::new(&accessed_targets, &cache_hit_targets, &sidecar_miss);
                                    let manifest = SidecarBenchmarkManifest {
                                        schema_version: 2,
                                        block_number: *block_number,
                                        block_hash: block.hash(),
                                        parent_hash,
                                        parent_state_root,
                                        cache_block: block_number - 1,
                                        cache_policy_metadata: format!(
                                            "LastNBlocks(account: {}, storage/code: {})",
                                            config.account_window, config.storage_window
                                        ),
                                        sidecar_file: sidecar_path
                                            .file_name()
                                            .map(|name| name.to_string_lossy().into_owned())
                                            .unwrap_or_else(|| sidecar_path.display().to_string()),
                                        sidecar_bytes: sidecar_bytes_len,
                                        cache_before: CacheFootprintStats::new(
                                            cache_snapshot_before.total_accounts,
                                            cache_snapshot_before.total_storage_slots,
                                            cache_snapshot_before.total_codes,
                                            cache_memory_before,
                                        ),
                                        cache_after: CacheFootprintStats::new(
                                            snapshot.total_accounts,
                                            snapshot.total_storage_slots,
                                            snapshot.total_codes,
                                            cache_memory_after,
                                        ),
                                        accessed: accessed_targets.clone(),
                                        cache_hit: cache_hit_targets.clone(),
                                        sidecar_miss,
                                        partition,
                                        full_sidecar_baseline_stats: full_sidecar_baseline_stats.clone(),
                                        partial_sidecar_stats: result.clone(),
                                        reduction: full_sidecar_baseline_stats
                                            .as_ref()
                                            .map(|full| WitnessReductionStats::new(&result, full)),
                                    };
                                    let manifest_path = sidecar_path.with_extension("manifest.json");
                                    let manifest_bytes = match serde_json::to_vec_pretty(&manifest) {
                                        Ok(bytes) => bytes,
                                        Err(e) => break 'sidecar Err(eyre::eyre!("Failed to serialize sidecar manifest: {:?}", e)),
                                    };
                                    if let Err(e) = fs::write(&manifest_path, manifest_bytes) {
                                        break 'sidecar Err(eyre::eyre!("Failed to write sidecar manifest {:?}: {:?}", manifest_path, e));
                                    }

                                    info!(
                                        target: "partial_stateless",
                                        block = *block_number,
                                        path = %sidecar_path.display(),
                                        manifest = %manifest_path.display(),
                                        size = format_bytes(sidecar_bytes_len),
                                        "Saved witness sidecar successfully"
                                    );

                                    Ok(())
                                };

                                if let Err(e) = sidecar_generation_result {
                                    warn!(
                                        target: "partial_stateless",
                                        block = *block_number,
                                        error = %e,
                                        "Sidecar generation failed (non-fatal)"
                                    );
                                }

                                Some(result)
                            }
                            Err(e) => {
                                warn!(
                                    target: "partial_stateless",
                                    block = *block_number,
                                    error = %e,
                                    "Failed to compute multiproof"
                                );
                                None
                            }
                        }
                    };

                    if let Some(witness) = witness_result {
                        info!(
                            target: "partial_stateless",
                            block = *block_number,
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
                            cpu_time_ms = witness.cpu_time_ms.unwrap_or(0),
                            major_page_faults = witness.major_page_faults.unwrap_or(0),
                            minor_page_faults = witness.minor_page_faults.unwrap_or(0),
                            "Witness size (Merkle proof)"
                        );
                    }

                    info!(
                        target: "partial_stateless",
                        block = *block_number,
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
                }

                // Save updated cache state to file
                if let Err(e) = save_to_file(&cache, &cache_path) {
                    warn!(
                        target: "partial_stateless",
                        block = tip_block,
                        error = %e,
                        "Failed to save cache state to disk"
                    );
                }
            }
            ExExNotification::ChainReorged { old, new } => {
                warn!(
                    target: "partial_stateless",
                    from_chain = ?old.range(),
                    to_chain = ?new.range(),
                    "Chain reorg detected — rolling back old blocks, then applying new chain"
                );

                // 1. Roll back the reverted (old) blocks newest→oldest, returning the
                //    cache to the fork point. On failure (history pruned/missing),
                //    cold-reset and let the new-chain loop below rebuild from scratch.
                let mut rollback_ok = true;
                for block_number in old.blocks().keys().rev() {
                    if let Err(e) = cache.rollback_block(*block_number) {
                        warn!(
                            target: "partial_stateless",
                            block = *block_number,
                            error = %e,
                            "Cache rollback failed on reorg — cold-resetting before reapply"
                        );
                        rollback_ok = false;
                        break;
                    }
                }
                if !rollback_ok {
                    cache.reset();
                }

                // 2. Apply the new canonical chain block-by-block (records undo).
                for (block_number, block) in new.blocks() {
                    let parent_block_number = block_number.saturating_sub(1);

                    let state_provider = match ctx.provider().history_by_block_number(parent_block_number) {
                        Ok(provider) => provider,
                        Err(e) => {
                            warn!(
                                target: "partial_stateless",
                                block = *block_number,
                                error = %e,
                                "Failed to get state provider for block parent on reorg. Skipping."
                            );
                            continue;
                        }
                    };

                    let state_provider_db = StateProviderDatabase::new(&state_provider);
                    let mut db = State::builder()
                        .with_bundle_update()
                        .with_database(state_provider_db)
                        .build();

                    let block_executor = ctx.evm_config().executor(&mut db);

                    let mut accessed = BlockAccessedState::default();
                    let sim_result = block_executor.execute_with_state_closure(block, |statedb: &State<_>| {
                        accessed = BlockAccessedState::from_simulated_state(statedb);
                    });

                    if let Err(e) = sim_result {
                        warn!(
                            target: "partial_stateless",
                            block = *block_number,
                            error = %e,
                            "Simulation failed on reorg. Skipping."
                        );
                        continue;
                    }

                    cache.on_block_executed(*block_number, &accessed);
                }

                // Persist the rebuilt cache: a restart before the next commit must
                // not reload the stale pre-reorg (old-branch) cache from disk.
                if let Err(e) = save_to_file(&cache, &cache_path) {
                    warn!(target: "partial_stateless", error = %e, "Failed to persist cache after reorg");
                }
            }
            ExExNotification::ChainReverted { old } => {
                warn!(
                    target: "partial_stateless",
                    reverted_chain = ?old.range(),
                    "Chain reverted — rolling back cache to the pre-revert state"
                );

                // Undo reverted blocks newest→oldest so each matches the top of the
                // undo stack. If a block can't be rolled back (history pruned/missing),
                // cold-reset: the cache rebuilds from subsequent canonical blocks.
                for block_number in old.blocks().keys().rev() {
                    if let Err(e) = cache.rollback_block(*block_number) {
                        warn!(
                            target: "partial_stateless",
                            block = *block_number,
                            error = %e,
                            "Cache rollback failed — cold-resetting cache"
                        );
                        cache.reset();
                        break;
                    }
                }

                // Persist the rolled-back cache: a restart before the next commit
                // must not reload the stale pre-revert cache from disk.
                if let Err(e) = save_to_file(&cache, &cache_path) {
                    warn!(target: "partial_stateless", error = %e, "Failed to persist cache after revert");
                }
            }
        }

        // Prune undo history below the finalized block: reorgs never cross
        // finality, so once a block is finalized its undo record is unreachable.
        // Keeping records down to finality means any legal reorg can be rolled
        // back precisely (this cache has no re-execution fallback — a missing
        // undo record forces a cold reset). When finality is unavailable (early
        // sync / no-finality chains) fall back to a fixed depth floor so the log
        // stays bounded. Mirrors reth's CHANGESET_CACHE_RETENTION_BLOCKS.
        const UNDO_LOG_FALLBACK_DEPTH: u64 = 64;
        let threshold = ctx
            .provider()
            .finalized_block_number()
            .ok()
            .flatten()
            .unwrap_or_else(|| cache.current_block().saturating_sub(UNDO_LOG_FALLBACK_DEPTH));
        cache.prune_undo_below(threshold);

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

/// Per-thread resource snapshot used to attribute multiproof cost.
///
/// Returns `(cpu_micros, major_faults, minor_faults)` for the CALLING thread
/// only. We use `RUSAGE_THREAD` (not `RUSAGE_SELF`) so the numbers isolate the
/// synchronous `multiproof` call from the rest of the node's threads (execution,
/// networking, DB writes) running concurrently while the node follows tip.
///
/// Diagnostic use: comparing the CPU delta against the wall-clock elapsed
/// separates compute-bound blocks (cpu ≈ wall) from I/O/wait-bound blocks
/// (cpu ≪ wall); a nonzero major-fault delta proves the cold trie read hit
/// disk/swap rather than the page cache — the signature of the environmental
/// tail. Linux-only; returns zeros elsewhere or if the syscall fails.
#[cfg(target_os = "linux")]
fn thread_rusage() -> (u64, u64, u64) {
    // SAFETY: `getrusage` only writes into the `rusage` we hand it; the struct
    // is fully zero-initialized before the call.
    unsafe {
        let mut ru: libc::rusage = std::mem::zeroed();
        if libc::getrusage(libc::RUSAGE_THREAD, &mut ru) != 0 {
            return (0, 0, 0);
        }
        let cpu_us = (ru.ru_utime.tv_sec as u64) * 1_000_000
            + (ru.ru_utime.tv_usec as u64)
            + (ru.ru_stime.tv_sec as u64) * 1_000_000
            + (ru.ru_stime.tv_usec as u64);
        (cpu_us, ru.ru_majflt as u64, ru.ru_minflt as u64)
    }
}

#[cfg(not(target_os = "linux"))]
fn thread_rusage() -> (u64, u64, u64) {
    (0, 0, 0)
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
